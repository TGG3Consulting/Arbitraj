//! Файл: tests/core_coin_by_address_review.rs
//! Что делает: решение 211, исправления по ревью — проверки на записанных справочниках сетей BingX
//! (tests/data/bingx_2026-10-24/config_getall.json, symbols.json) и Gate (cex_samples_2026_11_04/gate_spot_currencies.json):
//! К1 — нативный BNB бирж (решение 62: адрес обёртки по настройке) — одна монета у всех бирж, DEX-присутствие в её
//!      записи; BEP20-токен под адресом обёртки на другой бирже — отдельный актив (исключён NativeVsToken). В записях
//!      бирж BEP20-токена WBNB нет — для второго случая адрес обёртки в настройке (ввод человека, не данные площадки) —
//!      адрес NETMIND 0x03aa…e8d6: его листит Gate, а BingX — нет (как WBNB, который листит одна биржа);
//! К2 — credit_watch::same_asset: адрес только у одной стороны — разные активы;
//! В1 — у Gate один адрес под двумя тикерами (ETERNAL и ETERNAL0, записанный ответ): сеть берётся по тикеру, который
//!      выбрал справочник, а не первая запись с этим адресом;
//! В2 — Invalidated несёт адрес до ядра; ключ сверки балансов у DEX — адрес, у биржи — её тикер, у BNB — тикер;
//! В3 — подписка выключенной биржи с идущей сделкой: монета сделки — с тикером биржи, один раз;
//! В7 — котирование валюты: сначала площадка возникновения, на других — по их тикеру того же адреса; валюта без адреса
//!      — только площадка возникновения (кроме BNB газа).
//! Второе ревью (r2_*): п.1 — USDT без адреса = USDT ядра; п.2/п.5 — нативная и токен обёртки на одной бирже; п.6 —
//!      монета газа по тикеру И адресу; п.7 — котирование по адресу, BNB газа на всех биржах; п.8 — монета сделки
//!      главнее записи прежней подписки.
//! Настоящее ядро (Core::start) без сети. Моков нет.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core, arb::venues::{bingx, gate}, SQLite.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use alloy_primitives::{Address, address};
use arb::core::Core;
use arb::core::credit_watch::same_asset;
use arb::core::deps::Deps;
use arb::core::market_state::MarketState;
use arb::core::native::{self, Native};
use arb::core::registry::{self, CoinEntry, Exclusion, Inputs};
use arb::core::ticker_index::index_of;
use arb::core::types::{Asset, CoinNetworkInfo, Direction, Pool, VenueId};
use arb::core::venue_ticker as vt;
use arb::gateway::contract::VenueKind;
use arb::gateway::events::MarketEvent;
use arb::gateway::registry::Registry;
use arb::notify::dispatch::Notifier;
use arb::settings::store::Store;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use arb::venues::bingx::pairs::Snapshot;
use arb::venues::bingx::symbols;
use rust_decimal::Decimal;
use serde_json::Value;
use tokio::sync::broadcast;

const WBNB: Address = address!("0xbb4CdB9CBd36B01bD1cBaEBF2De08d9173bc095c");
const NMT: Address = address!("0x03aa6298f1370642642415edc0db8b957783e8d6");
const ETERNAL: Address = address!("0xD44FD09d74cd13838F137B590497595d6b3FEeA4");
/// USDT в BSC — из записи живого запуска (tests/data/live_2026-10-24/dex_subscription.json, поле usdt).
const USDT: Address = address!("0x55d398326f99059ff775485246999027b3197955");
/// Время ответа symbols в сыром журнале (мс) — tests/data/bingx_2026-10-24/README.md.
const SYMBOLS_AT_MS: i64 = 1792868538956;
/// Единственный адрес записанных справочников BingX и Gate под разными тикерами: BingX NIULAI, Gate 牛来.
const NIULAI: Address = address!("0xbeea1d618e533a387d941f58a7d4c9b7bd377777");

fn v(s: &str) -> VenueId {
    VenueId(s.into())
}

fn json_file(path: &str) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).expect("файл записи")).expect("JSON")
}

/// Сети BingX и Gate из записанных ответов (разбор адаптеров); с нативной монетой — native::apply, как registry_sync.
fn networks(n: Option<&Native>) -> HashMap<VenueId, Vec<CoinNetworkInfo>> {
    let gate = arb::venues::gate::networks_parse::parse_networks(
        &json_file("tests/data/cex_samples_2026_11_04/gate_spot_currencies.json"),
        &Value::Array(vec![]),
        "BSC",
        &v("gate"),
        false,
        &HashMap::new(),
        &|_| Some(2),
    )
    .unwrap()
    .list;
    let b = json_file("tests/data/bingx_2026-10-24/config_getall.json");
    let bingx =
        arb::venues::bingx::wallet::parse_networks(&b["data"], "BEP20", &v("bingx")).unwrap();
    let (gate, bingx) = match n {
        Some(n) => (
            native::apply(gate, Some("BSC"), n),
            native::apply(bingx, Some("BEP20"), n),
        ),
        None => (gate, bingx),
    };
    HashMap::from([(v("gate"), gate), (v("bingx"), bingx)])
}

fn kinds() -> Vec<(VenueId, VenueKind)> {
    vec![
        (v("bingx"), VenueKind::Cex),
        (v("gate"), VenueKind::Cex),
        (v("bsc_dex"), VenueKind::Dex),
    ]
}

/// Справочник пар BingX (записанный) для монет — так, как его шлёт адаптер (venues/bingx/pairs.rs).
fn bingx_pairs(assets: &[Asset]) -> MarketState {
    let list = json_file("tests/data/bingx_2026-10-24/symbols.json");
    let syms = symbols::Symbols::default();
    let at = chrono::DateTime::from_timestamp_millis(SYMBOLS_AT_MS).unwrap();
    syms.load(symbols::parse_list(&list["data"], "USDT").unwrap(), at);
    let mut m = MarketState::default();
    m.apply(Snapshot::build(&syms, assets).event(v("bingx"), at));
    m
}

/// Справочник ядра (тот же registry::build, что у пересборки).
fn coins(n: Option<&Native>, market: &MarketState) -> Vec<CoinEntry> {
    coins_of(networks(n), n, market)
}

/// Справочник ядра по готовым сетям бирж.
fn coins_of(
    nets: HashMap<VenueId, Vec<CoinNetworkInfo>>,
    n: Option<&Native>,
    market: &MarketState,
) -> Vec<CoinEntry> {
    let kinds = kinds();
    let networks = nets.into_iter().map(|(k, l)| (k, Arc::new(l))).collect();
    let quote = kinds
        .iter()
        .map(|(k, _)| (k.clone(), "USDT".to_string()))
        .collect();
    let bsc = HashMap::from([
        (v("bingx"), "BEP20".to_string()),
        (v("gate"), "BSC".to_string()),
    ]);
    let fresh = kinds.iter().map(|(k, _)| (k.clone(), Ok(()))).collect();
    let pool_ok = |_: &Pool| Ok(());
    let inter = HashMap::new();
    registry::build(&Inputs {
        venues: &kinds,
        networks: &networks,
        usdt: Some(USDT),
        quote: &quote,
        bsc_network: &bsc,
        pairs_fresh: &fresh,
        market,
        pool_ok: &pool_ok,
        intermediate: &inter,
        native: n,
    })
}

fn coin_at(list: &[CoinEntry], a: Address) -> &CoinEntry {
    list.iter()
        .find(|c| c.asset.bsc_contract == Some(a))
        .expect("адрес есть в справочнике")
}

fn native_vs_token(c: &CoinEntry, venue: &str) -> bool {
    c.presence.iter().any(|p| {
        p.venue.0 == venue
            && p.excluded
                .iter()
                .any(|e| matches!(e, Exclusion::NativeVsToken(_)))
    })
}

#[test]
fn k1_native_bnb_one_coin_across_cex_and_dex() {
    let n = Native {
        ticker: "BNB".into(),
        wrapped: WBNB,
    };
    let bnb = Asset {
        ticker: "BNB".into(),
        bsc_contract: Some(WBNB),
    };
    let list = coins(Some(&n), &bingx_pairs(&[bnb]));
    let c = coin_at(&list, WBNB);
    // (1) Нативный BNB обеих бирж — одна монета под тикером нативной; токена-обёртки в записях бирж нет.
    assert_eq!(c.asset.ticker, "BNB");
    for venue in ["bingx", "gate"] {
        let p = c.presence.iter().find(|p| p.venue.0 == venue).unwrap();
        assert_eq!(p.ticker, "BNB");
        assert!(!native_vs_token(c, venue), "{venue}: нативный BNB не токен");
    }
    // (2) DEX-присутствие — в записи нативной монеты (вывод нативного на кошелёк, продажа через пулы WBNB).
    assert!(c.presence.iter().any(|p| p.venue.0 == "bsc_dex"));
    assert_eq!(
        list.iter()
            .filter(|x| x.asset.bsc_contract == Some(WBNB))
            .count(),
        1
    );
}

#[test]
fn k1_wrapper_token_is_separate_asset() {
    // Без нативной монеты: у Gate NETMIND — обычная монета.
    let plain = coins(None, &MarketState::default());
    let c = coin_at(&plain, NMT);
    assert_eq!(c.asset.ticker, "NETMIND");
    assert!(!native_vs_token(c, "gate"));
    // Обёртка по настройке = этот адрес: BingX (адреса не листит) — нативный BNB под ним; Gate — BEP20-токен.
    let n = Native {
        ticker: "BNB".into(),
        wrapped: NMT,
    };
    let list = coins(Some(&n), &MarketState::default());
    let c = coin_at(&list, NMT);
    assert_eq!(
        c.asset.ticker, "BNB",
        "показное имя — нативной (is_native, GAS_TICKER)"
    );
    assert!(!native_vs_token(c, "bingx"));
    assert!(
        native_vs_token(c, "gate"),
        "токен обёртки на Gate — другой актив"
    );
    let g = c.presence.iter().find(|p| p.venue.0 == "gate").unwrap();
    assert_eq!(g.ticker, "NETMIND");
    assert!(!g.can_buy && !g.can_sell && !g.can_send && !g.can_receive);
    let gate_dirs: Vec<Direction> = c
        .directions()
        .into_iter()
        .filter(|d| d.buy_on.0 == "gate" || d.sell_on.0 == "gate")
        .collect();
    assert!(
        gate_dirs.is_empty(),
        "направлений нативный ↔ токен нет: {gate_dirs:?}"
    );
    // Свой нативный BNB Gate не сопоставлен (Gate листит адрес обёртки) — без адреса, отдельная запись.
    assert!(list.iter().any(|x| {
        x.asset.bsc_contract.is_none()
            && x.presence
                .iter()
                .any(|p| p.venue.0 == "gate" && p.ticker == "BNB")
    }));
}

#[test]
fn k2_address_on_one_side_is_other_asset() {
    let list = coins(None, &MarketState::default());
    let c = coin_at(&list, NIULAI).asset.clone();
    let raw = Asset {
        ticker: c.ticker.clone(),
        bsc_contract: None,
    };
    assert!(same_asset(&c, &c.clone()));
    assert!(!same_asset(&c, &raw), "адрес только у одной стороны");
    assert!(!same_asset(&raw, &c));
    assert!(
        same_asset(&raw, &raw.clone()),
        "без адреса у обоих — по тикеру"
    );
    assert_eq!(same_asset(&c, &raw), c == raw, "как Asset::eq");
}

/// Папка теста (новая).
fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("arb-review211-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Ядро без площадок в шлюзе; справочник сетей, виды площадок и карта тикеров — по записанным сетям.
async fn start(d: &Path, list: &[CoinEntry]) -> Core {
    start_with(d, list, &[]).await
}

/// То же, с настройками площадок (площадка, имя, значение), заданными до запуска ядра — пересборка от их изменения
/// не перетирает карту тикеров теста.
async fn start_with(d: &Path, list: &[CoinEntry], pre: &[(&str, &str, Value)]) -> Core {
    let db = Db::open(&d.join("t.sqlite")).unwrap();
    let (changes, _) = broadcast::channel(64);
    let (jtx, _) = broadcast::channel(64);
    let journal = Journal::new(db.clone(), jtx);
    let settings = Store::load(db.clone(), changes.clone(), journal.clone())
        .await
        .unwrap();
    for (venue, name, value) in pre {
        let at = arb::settings::scope::Scope::Venue(venue.to_string());
        settings
            .set(name, at, Some(value.clone()), "тест")
            .await
            .unwrap();
    }
    let vault = arb::secrets::Vault::locked(d.join("secrets.enc"));
    let notifier = Notifier {
        http: reqwest::Client::new(),
        settings: settings.clone(),
        vault,
        journal: journal.clone(),
    };
    let core = Core::start(Deps {
        gw: Registry::new(),
        settings,
        journal,
        notifier,
        db,
        changes,
        buffer: 64,
        data_dir: d.to_path_buf(),
    })
    .await;
    *core.inner.kinds.write().unwrap() = kinds();
    *core.inner.tickers.write().unwrap() = index_of(list);
    core
}

#[tokio::test]
async fn v1_v2_v3_v7_by_address_and_venue_ticker() {
    let tmp = dir("e2e");
    let list = coins(None, &MarketState::default());
    let core = start(&tmp, &list).await;
    // В1: у Gate ETERNAL и ETERNAL0 — один адрес; справочник выбрал один тикер. Записи сетей переставлены так, что
    // первой с этим адресом идёт ДРУГОЙ тикер — сеть всё равно берётся по выбранному.
    let eternal = coin_at(&list, ETERNAL).asset.clone();
    let chosen = vt::ticker_on(&core, &eternal, &v("gate"));
    let mut gate = networks(None).remove(&v("gate")).unwrap();
    gate.sort_by_key(|i| i.asset.ticker != chosen);
    gate.reverse();
    let first = gate
        .iter()
        .find(|i| i.asset.bsc_contract == Some(ETERNAL))
        .unwrap();
    assert_ne!(
        first.asset.ticker, chosen,
        "первая запись адреса — другой тикер"
    );
    core.inner
        .networks
        .write()
        .unwrap()
        .insert(v("gate"), Arc::new(gate));
    let net = arb::core::dirctx_venue::network(&core, &v("gate"), &eternal, "BSC").unwrap();
    assert_eq!(net.asset.ticker, chosen);

    // В2: событие DEX несёт адрес до ядра.
    let mut m = MarketState::default();
    let t = m.apply(MarketEvent::Invalidated {
        venue: v("bsc_dex"),
        ticker: "NIULAI".into(),
        address: Some(NIULAI),
    });
    assert!(
        matches!(t, arb::core::touched::Touched::Invalidated { address: Some(a), .. } if a == NIULAI),
        "{t:?}"
    );
    // Ключ сверки балансов: DEX — адрес, биржа — её тикер; монета газа — тикер (строка BNB несёт WBNB в locked).
    let coin = coin_at(&list, NIULAI).asset.clone();
    let key = arb::core::reconcile_key::key;
    assert_eq!(key(&core, &coin, &v("bsc_dex")), format!("{NIULAI:#x}"));
    assert_eq!(key(&core, &coin, &v("bingx")), "NIULAI");
    assert_eq!(key(&core, &coin, &v("gate")), "牛来");
    let bnb = Asset {
        ticker: "BNB".into(),
        bsc_contract: Some(WBNB),
    };
    set_native(&core, WBNB).await;
    assert_eq!(key(&core, &bnb, &v("bsc_dex")), "BNB");
    // Второе ревью, п.6: токен с тикером BNB, но другим адресом — не монета газа, ключ — адрес.
    let fake = Asset {
        ticker: "BNB".into(),
        bsc_contract: Some(NIULAI),
    };
    assert_eq!(key(&core, &fake, &v("bsc_dex")), format!("{NIULAI:#x}"));
    assert!(arb::core::reconcile_key::is_native_key(&core, "BNB"));
    assert!(!arb::core::reconcile_key::is_native_key(
        &core,
        &format!("{NIULAI:#x}")
    ));

    // В3: биржа выключена, по ней идёт сделка с монетой под показным именем — подписке тикер биржи, один раз.
    let shown = Asset {
        ticker: "牛来".into(),
        bsc_contract: Some(NIULAI),
    };
    let dir = Direction {
        asset: shown.clone(),
        buy_on: v("bingx"),
        sell_on: v("bsc_dex"),
    };
    for id in ["d1", "d2"] {
        let deal = arb::core::deal::Deal::new(id.into(), dir.clone(), Value::Null, Decimal::ZERO);
        core.inner.deals.lock().unwrap().insert(id.into(), deal);
    }
    {
        let on = vt::on_venue(&core, &shown, &v("bingx"));
        let mut subs = core.inner.subs.lock().unwrap();
        subs.begin(&v("bingx"), std::slice::from_ref(&on));
        subs.done(&v("bingx"), vec![on], Ok(()));
    }
    let (want, held) = arb::core::feed_hold::want_disabled(&core, &v("bingx"));
    assert!(held);
    assert_eq!(want.len(), 1, "{want:?}");
    assert_eq!(
        (want[0].ticker.as_str(), want[0].bsc_contract),
        ("NIULAI", Some(NIULAI))
    );

    // В7: валюта с адресом — площадка возникновения, затем другие биржи по их тикеру этого адреса.
    let cex = [v("bingx"), v("gate")];
    let plan = arb::core::convert_at::plan;
    assert_eq!(
        plan(&core, &cex, &v("gate"), "牛来", None),
        vec![(v("gate"), "牛来".into()), (v("bingx"), "NIULAI".into())]
    );
    // Тикер Gate на BingX — не монета BingX: без адреса — только площадка возникновения.
    assert_eq!(
        plan(&core, &cex, &v("bingx"), "牛来", None),
        vec![(v("bingx"), "牛来".into())]
    );
    // BNB газа — нативная монета сети, одна у всех: на всех биржах, площадка возникновения первой.
    assert_eq!(
        plan(&core, &cex, &v("gate"), "BNB", None),
        vec![(v("gate"), "BNB".into()), (v("bingx"), "BNB".into())]
    );
    // Второе ревью, п.7: BNB газа с адресом обёртки — тоже на всех биржах их тикером газа; токен «BNB» с другим
    // адресом — не газ: по тикеру этого адреса.
    assert_eq!(
        plan(&core, &cex, &v("gate"), "BNB", Some(WBNB)),
        vec![(v("gate"), "BNB".into()), (v("bingx"), "BNB".into())]
    );
    assert_eq!(
        plan(&core, &cex, &v("gate"), "BNB", Some(NIULAI)),
        vec![(v("gate"), "BNB".into()), (v("bingx"), "NIULAI".into())]
    );
    // DEX — монета по адресу, не по показному имени (имя DEX любое — решает адрес).
    let cex3 = [v("bingx"), v("gate")];
    assert_eq!(
        plan(&core, &cex3, &v("bsc_dex"), "ЧТО-УГОДНО", Some(NIULAI)),
        vec![(v("bingx"), "NIULAI".into()), (v("gate"), "牛来".into())]
    );
    let _ = std::fs::remove_dir_all(&tmp);
}

/// Нативная монета в настройках DEX (ввод человека в интерфейсе: dex.native_ticker, dex.native_wrapped).
async fn set_native(core: &Core, wrapped: Address) {
    let s = &core.inner.deps.settings;
    let at = arb::settings::scope::Scope::Venue("bsc_dex".into());
    s.set(
        "dex.native_ticker",
        at.clone(),
        Some(serde_json::json!("BNB")),
        "тест",
    )
    .await
    .unwrap();
    s.set(
        "dex.native_wrapped",
        at,
        Some(serde_json::json!(format!("{wrapped:#x}"))),
        "тест",
    )
    .await
    .unwrap();
}

/// Второе ревью, п.2 и п.5: нативный BNB (решение 62: адрес обёртки) и BEP20-токен обёртки на ОДНОЙ бирже. У Gate
/// в записанном ответе свой нативный BNB (без адреса) и NETMIND (адрес NMT); обёртка по настройке — NMT, и нативной
/// Gate поставлен этот адрес (как сделал бы native::apply) — обе записи Gate под одним адресом.
#[test]
fn r2_native_and_token_on_same_venue() {
    let n = Native {
        ticker: "BNB".into(),
        wrapped: NMT,
    };
    let mut nets = networks(None);
    for i in nets.get_mut(&v("gate")).unwrap().iter_mut() {
        if i.asset.ticker == "BNB" && i.asset.bsc_contract.is_none() {
            i.asset.bsc_contract = Some(NMT);
        }
    }
    let list = coins_of(nets, Some(&n), &MarketState::default());
    let c = coin_at(&list, NMT);
    assert_eq!(c.asset.ticker, "BNB");
    let gate: Vec<_> = c.presence.iter().filter(|p| p.venue.0 == "gate").collect();
    assert_eq!(gate.len(), 1, "одна запись биржи: {gate:?}");
    // Торгуется нативная; конфликта тикеров нет.
    assert_eq!(gate[0].ticker, "BNB");
    assert!(
        !gate[0].excluded.iter().any(|e| matches!(
            e,
            Exclusion::TickerConflict(_) | Exclusion::NativeVsToken(_)
        )),
        "{:?}",
        gate[0].excluded
    );
    // П.5: исключённый токен не в картах переводчика — тикер Gate «NETMIND» не переводится в монету нативной.
    let idx = index_of(&list);
    assert_eq!(
        idx.get(&v("gate"), "BNB").map(|a| a.bsc_contract),
        Some(Some(NMT))
    );
    assert!(idx.get(&v("gate"), "NETMIND").is_none());
    // Нативная только у BingX, токен обёртки у Gate (исключён NativeVsToken) — его тикер тоже не в картах.
    let n2 = Native {
        ticker: "BNB".into(),
        wrapped: NMT,
    };
    let list2 = coins(Some(&n2), &MarketState::default());
    let idx2 = index_of(&list2);
    assert!(native_vs_token(coin_at(&list2, NMT), "gate"));
    assert!(idx2.get(&v("gate"), "NETMIND").is_none());
}

/// Второе ревью, п.1: событие баланса USDT биржи без адреса — переводится в USDT ядра (адрес из справочника бирж) и
/// закрывает ожидание перегона USDT (usdt_return.rs: тикер venue.quote_asset, адрес USDT ядра), а не «внешнее
/// изменение».
#[tokio::test]
async fn r2_usdt_event_without_address_matches_relay() {
    let tmp = dir("usdt");
    let list = coins(None, &MarketState::default());
    let pre = [
        ("bingx", "venue.quote_asset", serde_json::json!("USDT")),
        (
            "bingx",
            "transfer.credit_amount_tolerance_pct",
            serde_json::json!("0.5"),
        ),
    ];
    let core = start_with(&tmp, &list, &pre).await;
    *core.inner.usdt.write().unwrap() = Ok(USDT);
    let expect = Asset {
        ticker: "USDT".into(),
        bsc_contract: Some(USDT),
    };
    let raw = Asset {
        ticker: "USDT".into(),
        bsc_contract: None,
    };
    assert_eq!(vt::asset_back(&core, &v("bingx"), raw.clone()), expect);
    let amount = Decimal::new(100, 0);
    let t =
        arb::core::credit_watch::register(&core, "d1", &v("bsc_dex"), &v("bingx"), &expect, amount);
    arb::core::credit_event::on_change(
        &core,
        arb::core::types::BalanceChange {
            venue: v("bingx"),
            asset: raw,
            delta: amount,
            kind: arb::core::types::BalanceKind::Deposit,
            own: false,
            tx_hash: None,
            at: chrono::Utc::now(),
            source: "тест".into(),
            account: None,
        },
    )
    .await;
    assert!(t.has_event(), "поступление USDT прикреплено к перегону");
    let _ = std::fs::remove_dir_all(&tmp);
}

/// Второе ревью, п.8: подписка выключенной биржи — при том же адресе остаётся монета сделки с текущим тикером
/// площадки, а не запись прежней подписки со старым тикером.
#[tokio::test]
async fn r2_feed_hold_prefers_deal_coin() {
    let tmp = dir("hold");
    let list = coins(None, &MarketState::default());
    let core = start(&tmp, &list).await;
    let coin = coin_at(&list, NIULAI).asset.clone();
    let dir_ = Direction {
        asset: coin.clone(),
        buy_on: v("bingx"),
        sell_on: v("bsc_dex"),
    };
    let deal = arb::core::deal::Deal::new("d1".into(), dir_, Value::Null, Decimal::ZERO);
    core.inner.deals.lock().unwrap().insert("d1".into(), deal);
    let old = Asset {
        ticker: "NIULAI_OLD".into(),
        bsc_contract: Some(NIULAI),
    };
    {
        let mut subs = core.inner.subs.lock().unwrap();
        subs.begin(&v("bingx"), std::slice::from_ref(&old));
        subs.done(&v("bingx"), vec![old], Ok(()));
    }
    let (want, held) = arb::core::feed_hold::want_disabled(&core, &v("bingx"));
    assert!(held);
    assert_eq!(want.len(), 1, "{want:?}");
    assert_eq!(want[0].ticker, "NIULAI");
    let _ = std::fs::remove_dir_all(&tmp);
}

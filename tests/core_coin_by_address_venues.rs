//! Файл: tests/core_coin_by_address_venues.rs
//! Что делает: решение 211, фаза 2a — сквозная проверка ядра на записанных справочниках сетей BingX
//! (tests/data/bingx_2026-10-24/config_getall.json) и Gate (cex_samples_2026_11_04/gate_spot_currencies.json).
//! Случай NMT/NETMIND (0x03aa…e8d6) из живых данных 07.11: в записи BingX 2026-10-24 монеты NMT нет, поэтому сквозной
//! сценарий идёт на единственном адресе этих записей под разными тикерами — 0xbeea…7777 (BingX NIULAI, Gate 牛来);
//! NETMIND проверяется на стороне Gate (одна монета 0x03aa…, тикер Gate). Данные не придумываются (моков нет).
//! Настоящее ядро (Core::start) и настоящий адаптер BingX без сети (ключей нет — хранилище секретов заблокировано),
//! записанный справочник пар BingX и записанный стакан CAKE. Моков нет. Проверяется:
//! (1) справочник ядра: одна монета, у каждой биржи свой тикер (Presence::ticker — по нему подписка бирже, subscribe.rs
//!     wanted); переводчик ядра on_venue / ticker_on даёт тикер площадки и тот же адрес;
//! (2) вызовы шлюза из ядра уходят с тикером площадки: котировка (trade_ops::sell_value) и ордер продажи
//!     (balance_sell_check::request — стакан, пара и запрос ордера) доходят до BingX с «NMT», а не с показным именем;
//! (3) ответ площадки и события с её тикером переводятся в монету ядра: событие баланса без адреса (asset_back) —
//!     та же монета для сверки зачислений (credit_watch::same_asset), затраты «в монете» (costs_back) и fee_asset
//!     (name_back) — в имя ядра; чужой тикер (USDT) — как есть;
//! (4) монета с одинаковым тикером без перевода (CAKE) не меняется: стакан находится, запрос ордера собирается.
//! Вывод и адрес депозита идут через тот же on_venue (core/transfers.rs); их ответов BingX в записях нет, а площадка-
//! заглушка запрещена (CLAUDE.md) — проверяется переводчик, которым они пользуются.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core (venue_ticker, registry, ticker_index, trade_ops, balance_sell_check, credit_watch),
//! arb::venues::{bingx, gate}, SQLite.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;

use alloy_primitives::Address;
use arb::core::Core;
use arb::core::credit_watch::same_asset;
use arb::core::deps::Deps;
use arb::core::market_state::MarketState;
use arb::core::registry::{self, Inputs};
use arb::core::ticker_index::index_of;
use arb::core::types::{Asset, CoinNetworkInfo, Cost, CostKind, Pool, VenueId};
use arb::core::venue_ticker as vt;
use arb::gateway::contract::VenueKind;
use arb::gateway::events::MarketEvent;
use arb::gateway::registry::Registry;
use arb::notify::dispatch::Notifier;
use arb::settings::store::Store;
use arb::settings::value::Scope;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use arb::venues::bingx::depth::{self, Msg};
use arb::venues::bingx::{Bingx, symbols};
use chrono::DateTime;
use rust_decimal::Decimal;
use serde_json::{Value, json};
use tokio::sync::broadcast;

/// Время ответа symbols в сыром журнале (мс) — tests/data/bingx_2026-10-24/README.md.
const SYMBOLS_AT_MS: i64 = 1792868538956;
/// Монета из живого разбора 07.11 (bingx NMT = gate NETMIND): в записи BingX 2026-10-24 её нет — проверяется Gate.
const NMT_ADDR: &str = "0x03aa6298f1370642642415edc0db8b957783e8d6";
/// Единственный адрес записанных справочников BingX и Gate под разными тикерами: BingX NIULAI, Gate 牛来.
const ADDR: &str = "0xbeea1d618e533a387d941f58a7d4c9b7bd377777";
const BT: &str = "NIULAI";
const GT: &str = "牛来";

fn d(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

fn v(s: &str) -> VenueId {
    VenueId(s.into())
}

fn json_file(path: &str) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).expect("файл записи")).expect("JSON")
}

/// Папка теста (новая).
fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("arb-coinaddr-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Сети BingX и Gate из записанных ответов.
fn networks() -> HashMap<VenueId, Vec<CoinNetworkInfo>> {
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
    HashMap::from([(v("gate"), gate), (v("bingx"), bingx)])
}

/// Справочник монет ядра по записанным сетям (тот же registry::build, что у пересборки).
fn coins() -> Vec<arb::core::registry::CoinEntry> {
    let n = networks();
    let kinds = vec![(v("bingx"), VenueKind::Cex), (v("gate"), VenueKind::Cex)];
    let networks = n
        .iter()
        .map(|(k, l)| (k.clone(), Arc::new(l.clone())))
        .collect();
    let quote = kinds
        .iter()
        .map(|(k, _)| (k.clone(), "USDT".to_string()))
        .collect();
    let bsc = HashMap::from([
        (v("bingx"), "BEP20".to_string()),
        (v("gate"), "BSC".to_string()),
    ]);
    let fresh = kinds.iter().map(|(k, _)| (k.clone(), Ok(()))).collect();
    let market = MarketState::default();
    let pool_ok = |_: &Pool| Ok(());
    let inter = HashMap::new();
    registry::build(&Inputs {
        venues: &kinds,
        networks: &networks,
        usdt: None,
        quote: &quote,
        bsc_network: &bsc,
        pairs_fresh: &fresh,
        market: &market,
        pool_ok: &pool_ok,
        intermediate: &inter,
        native: None,
    })
}

/// Первый записанный стакан CAKE-USDT BingX (монета — как пришла от адаптера).
fn cake_book(cake: Asset) -> arb::core::types::OrderBook {
    let raw = std::fs::read_to_string("tests/data/bingx/ws_spot_2026-10-23.jsonl").unwrap();
    for line in raw.lines() {
        let m: Value = serde_json::from_str(line).unwrap();
        if let Msg::Depth {
            symbol,
            bids,
            asks,
            exchange_time,
        } = depth::parse(m["m"].as_str().unwrap())
            && symbol == "CAKE-USDT"
        {
            return depth::to_book(cake, bids, asks, exchange_time);
        }
    }
    panic!("в записи нет стакана CAKE-USDT")
}

/// Ядро с настоящим адаптером BingX (справочник пар — записанный) и переводчиком тикеров из записанных сетей.
async fn start(d: &Path) -> Core {
    let db = Db::open(&d.join("t.sqlite")).unwrap();
    let (changes, _) = broadcast::channel(64);
    let (jtx, _) = broadcast::channel(64);
    let journal = Journal::new(db.clone(), jtx);
    let settings = Store::load(db.clone(), changes.clone(), journal.clone())
        .await
        .unwrap();
    let sv = Scope::Venue("bingx".into());
    for (n, val) in [
        ("venue.quote_asset", json!("USDT")),
        ("venue.symbols_refresh_s", json!(1_000_000_000)),
        ("venue.order_time_in_force", json!("IOC")),
        ("feed.stale_after_ms", json!(3000)),
        // Решение 238: обязательна у всех бирж (котировка исполнения — core/exec_book.rs).
        ("venue.book_stale_ms", json!(3000)),
    ] {
        settings
            .set(n, sv.clone(), Some(val), "тест")
            .await
            .unwrap();
    }
    let vault = arb::secrets::Vault::locked(d.join("secrets.enc"));
    let ctx = arb::venues::bingx::ctx::Ctx {
        settings: settings.clone(),
        vault: vault.clone(),
        http: reqwest::Client::new(),
        journal: journal.clone(),
        raw: arb::storage::raw_log::RawLog::start(d.to_path_buf(), settings.clone()),
        taps: Default::default(),
    };
    let bx = Arc::new(Bingx::new(ctx));
    let list = json_file("tests/data/bingx_2026-10-24/symbols.json");
    bx.symbols().load(
        symbols::parse_list(&list["data"], "USDT").unwrap(),
        DateTime::from_timestamp_millis(SYMBOLS_AT_MS).unwrap(),
    );
    let gw = Registry::new();
    gw.register(bx).await;
    let notifier = Notifier {
        http: reqwest::Client::new(),
        settings: settings.clone(),
        vault,
        journal: journal.clone(),
    };
    let core = Core::start(Deps {
        gw,
        settings,
        journal,
        notifier,
        db,
        changes,
        buffer: 64,
        data_dir: d.to_path_buf(),
    })
    .await;
    *core.inner.tickers.write().unwrap() = index_of(&coins());
    core
}

/// Монета справочника ядра по адресу.
fn coin_at(list: &[arb::core::registry::CoinEntry], a: Address) -> arb::core::registry::CoinEntry {
    list.iter()
        .find(|c| c.asset.bsc_contract == Some(a))
        .expect("адрес есть в записанных сетях")
        .clone()
}

#[tokio::test]
async fn one_address_two_tickers_end_to_end() {
    let tmp = dir("e2e");
    let core = start(&tmp).await;
    let list = coins();
    let addr: Address = ADDR.parse().unwrap();
    let c = coin_at(&list, addr);
    let coin = c.asset.clone();

    // (1) Одна монета, у каждой биржи свой тикер — по нему подписка (subscribe.rs wanted берёт Presence::ticker).
    let at = |venue: &str| c.presence.iter().find(|p| p.venue.0 == venue).unwrap();
    assert_eq!(at("bingx").ticker, BT);
    assert_eq!(at("gate").ticker, GT);
    for (venue, t) in [("bingx", BT), ("gate", GT)] {
        let on = vt::on_venue(&core, &coin, &v(venue));
        assert_eq!((on.ticker.as_str(), on.bsc_contract), (t, Some(addr)));
        // Монета ядра под ЛЮБЫМ показным именем (имя не ключ) — тот же тикер площадки.
        for name in [BT, GT] {
            let shown = Asset {
                ticker: name.into(),
                bsc_contract: Some(addr),
            };
            assert_eq!(vt::ticker_on(&core, &shown, &v(venue)), t);
        }
    }
    // Баланс биржи — по её тикеру (у DEX — по адресу, venue_ticker::balance_key).
    assert_eq!(vt::balance_key(&core, &coin, &v("bingx")), BT);
    // NMT/NETMIND: у Gate одна монета 0x03aa… с тикером Gate.
    let nmt = coin_at(&list, NMT_ADDR.parse().unwrap());
    assert_eq!(vt::ticker_on(&core, &nmt.asset, &v("gate")), "NETMIND");

    // (2) Котировка из ядра доходит до BingX с её тикером, даже когда показное имя — тикер Gate (стакана в записи
    // нет — ответ адаптера называет монету).
    let shown = Asset {
        ticker: GT.into(),
        bsc_contract: Some(addr),
    };
    let err = arb::core::trade_ops::sell_value(&core, None, &v("bingx"), &shown, d_qty())
        .await
        .unwrap_err();
    assert!(
        err.contains(&format!("BingX {BT}")),
        "котировка ушла не с тикером BingX: {err}"
    );
    assert!(!err.contains(GT), "в BingX ушло показное имя: {err}");
    // Ордер продажи: стакан и пара ищутся по тикеру BingX.
    let err = arb::core::balance_sell_check::request(&core, &v("bingx"), &shown, d_qty(), "t")
        .unwrap_err();
    assert!(
        err.contains(&format!("{BT}/USDT")),
        "стакан/пара не по тикеру BingX: {err}"
    );
    assert!(!err.contains(GT), "{err}");

    // (3) Событие BingX без адреса (баланс, перевод) — монета ядра; без перевода тикер не совпал бы.
    let raw = Asset {
        ticker: BT.into(),
        bsc_contract: None,
    };
    let back = vt::asset_back(&core, &v("bingx"), raw.clone());
    assert_eq!(back, coin);
    assert_eq!(back.bsc_contract, Some(addr));
    assert!(same_asset(&shown, &back), "зачисление BingX не узнано");
    assert!(
        !same_asset(&shown, &raw) || coin.ticker == BT,
        "сравнение без перевода не должно узнавать"
    );
    let gate_raw = Asset {
        ticker: GT.into(),
        bsc_contract: None,
    };
    assert_eq!(vt::asset_back(&core, &v("gate"), gate_raw), coin);
    // Тикер BingX на Gate ничего не значит — не наша монета.
    let foreign = vt::asset_back(&core, &v("gate"), raw.clone());
    assert_ne!(foreign.bsc_contract, Some(addr));
    // USDT и монеты вне справочника — как есть.
    let usdt = Asset {
        ticker: "USDT".into(),
        bsc_contract: None,
    };
    assert_eq!(vt::asset_back(&core, &v("bingx"), usdt.clone()), usdt);
    // Затраты и fee_asset в тикере площадки — в имя ядра; USDT, BNB — как есть.
    let costs = vec![
        Cost::new(CostKind::WithdrawFee, d("1"), BT, false),
        Cost::new(CostKind::TradeFee, d("0.1"), "USDT", false),
    ];
    let named = vt::costs_back(&core, &coin, &v("bingx"), costs);
    assert_eq!(named[0].asset, coin.ticker);
    assert_eq!(named[1].asset, "USDT");
    assert_eq!(vt::name_back(&core, &coin, &v("gate"), GT), coin.ticker);
    assert_eq!(vt::name_back(&core, &coin, &v("gate"), "BNB"), "BNB");

    // (4) Монета с тем же тикером на бирже (CAKE): стакан BingX находится, запрос ордера собирается.
    let cake = list
        .iter()
        .find(|c| {
            c.presence
                .iter()
                .any(|p| p.venue.0 == "bingx" && p.ticker == "CAKE")
        })
        .expect("CAKE в записанных сетях BingX")
        .asset
        .clone();
    assert_eq!(vt::ticker_on(&core, &cake, &v("bingx")), "CAKE");
    core.inner
        .market
        .write()
        .unwrap()
        .apply(MarketEvent::Book(cake_book(vt::on_venue(
            &core,
            &cake,
            &v("bingx"),
        ))));
    let req = arb::core::balance_sell_check::request(&core, &v("bingx"), &cake, d("5"), "t")
        .expect("запрос ордера CAKE");
    assert_eq!(req.asset.ticker, "CAKE");
    assert_eq!(req.asset.bsc_contract, cake.bsc_contract);
    let _ = std::fs::remove_dir_all(&tmp);
}

fn d_qty() -> Decimal {
    d("10")
}

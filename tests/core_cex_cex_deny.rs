//! Файл: tests/core_cex_cex_deny.rs
//! Что делает: решение 248 на настоящем ядре и SQLite во временной папке (площадок в реестре нет, моков логики нет;
//! запись справочника монеты — сеть BEP20 BingX из записанного config_getall, tests/size_common). Проверяется:
//! (1) список venue.cex_cex_deny односторонний: у binance [bingx] — направление bingx → binance есть, binance → bingx
//! нет; у DEX направления не трогаются; (2) venue.cex_cex = нет — ни одного направления биржа ↔ биржа с биржей в обе
//! стороны (справочник и risk-проверка cex_link::direction_ok); (3) кейс Тиграна 10.11: сделка bingx → bsc_dex, монета на
//! кошельке DEX, у mexc venue.cex_cex = нет — «Перегон» (relay::targets) и «Продать и перегнать»
//! (manual_relay_targets::targets) mexc не выбирают; (4) список биржи покупки запрещает перегон на биржу из списка,
//! список биржи-цели — нет; (5) не задано — цели как до решения.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core, arb::gateway, arb::settings, arb::storage, arb::notify, arb::secrets, tests/size_common.

mod size_common;

use arb::core::Core;
use arb::core::cex_link;
use arb::core::deal::{Deal, Step};
use arb::core::deps::Deps;
use arb::core::registry::{CoinEntry, Presence};
use arb::core::types::{DexAccountId, Direction, VenueId};
use arb::gateway::contract::VenueKind;
use arb::gateway::registry::Registry;
use arb::notify::dispatch::Notifier;
use arb::settings::store::Store;
use arb::settings::value::Scope;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use rust_decimal::Decimal;
use serde_json::{Value, json};
use size_common::Rec;
use tokio::sync::broadcast;

/// Ядро на базе во временной папке. Возвращает ядро и настройки.
async fn start(name: &str) -> (Core, Store) {
    let d = std::env::temp_dir().join(format!("arb-248-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let db = Db::open(&d.join("t.sqlite")).unwrap();
    let (changes, _) = broadcast::channel(64);
    let (jtx, _) = broadcast::channel(64);
    let journal = Journal::new(db.clone(), jtx);
    let settings = Store::load(db.clone(), changes.clone(), journal.clone())
        .await
        .unwrap();
    let notifier = Notifier {
        http: reqwest::Client::new(),
        settings: settings.clone(),
        vault: arb::secrets::Vault::locked(d.join("secrets.enc")),
        journal: journal.clone(),
    };
    let core = Core::start(Deps {
        gw: Registry::new(),
        settings: settings.clone(),
        journal,
        notifier,
        db,
        changes,
        buffer: 64,
        data_dir: d.clone(),
    })
    .await;
    (core, settings)
}

fn v(id: &str) -> VenueId {
    VenueId(id.into())
}

/// Задаёт настройку площадки (None — очистить).
async fn venue_set(s: &Store, name: &str, id: &str, val: Option<Value>) {
    s.set(name, Scope::Venue(id.into()), val, "тест")
        .await
        .unwrap();
}

/// Запись справочника: биржи (сеть BEP20 из записи BingX) и bsc_dex.
fn entry(rec: &Rec, cex: &[&str]) -> CoinEntry {
    let p = |id: &str, info| Presence {
        venue: v(id),
        ticker: "CAKE".into(),
        network: None,
        can_receive: true,
        can_send: true,
        can_buy: true,
        can_sell: true,
        excluded: vec![],
        info,
        via: None,
    };
    let mut presence: Vec<Presence> = cex.iter().map(|id| p(id, Some(rec.net.clone()))).collect();
    presence.push(p("bsc_dex", None));
    CoinEntry {
        asset: size_common::calc::direction().asset,
        presence,
        block: None,
        problem: None,
        wait: None,
        cex_cex: None,
        cex_cex_off: vec![],
        cex_cex_deny: vec![],
    }
}

/// Поля решений 214 и 248 — тем же путём, что пересборка (rebuild_blacklist.rs: cex_link::on, cex_link::deny_pairs).
fn fill(s: &Store, c: &mut CoinEntry) {
    c.cex_cex_off = c
        .presence
        .iter()
        .filter(|p| p.info.is_some() && !cex_link::on(s, &p.venue.0))
        .map(|p| p.venue.clone())
        .collect();
    c.cex_cex_deny = cex_link::deny_pairs(s, &c.presence);
}

/// Направления монеты парами (покупка, продажа).
fn pairs(c: &CoinEntry) -> Vec<(String, String)> {
    c.directions()
        .into_iter()
        .map(|d| (d.buy_on.0, d.sell_on.0))
        .collect()
}

fn has(p: &[(String, String)], a: &str, b: &str) -> bool {
    p.iter().any(|(x, y)| x == a && y == b)
}

/// Виды площадок в ядре (биржи и bsc_dex).
fn kinds(core: &Core) {
    *core.inner.kinds.write().unwrap() = vec![
        (v("bingx"), VenueKind::Cex),
        (v("mexc"), VenueKind::Cex),
        (v("binance"), VenueKind::Cex),
        (v("bsc_dex"), VenueKind::Dex),
    ];
}

fn dir(a: &str, b: &str) -> Direction {
    Direction {
        asset: size_common::calc::direction().asset,
        buy_on: v(a),
        sell_on: v(b),
    }
}

/// (1) Список односторонний: у binance [bingx] — bingx → binance есть, binance → bingx нет.
#[tokio::test]
async fn deny_list_is_one_way() {
    let (core, s) = start("oneway").await;
    kinds(&core);
    let rec = Rec::load("cex248-oneway").await;
    let mut c = entry(&rec, &["bingx", "binance"]);
    fill(&s, &mut c);
    let all = pairs(&c);
    assert!(
        has(&all, "bingx", "binance") && has(&all, "binance", "bingx"),
        "{all:?}"
    );
    venue_set(&s, cex_link::DENY, "binance", Some(json!(["bingx"]))).await;
    fill(&s, &mut c);
    let got = pairs(&c);
    assert!(has(&got, "bingx", "binance"), "{got:?}");
    assert!(!has(&got, "binance", "bingx"), "{got:?}");
    for (a, b) in [
        ("bingx", "bsc_dex"),
        ("bsc_dex", "bingx"),
        ("binance", "bsc_dex"),
        ("bsc_dex", "binance"),
    ] {
        assert!(has(&got, a, b), "нет {a} → {b}: {got:?}");
    }
    assert!(cex_link::direction_ok(&core, &dir("bingx", "binance")).is_ok());
    let why = cex_link::direction_ok(&core, &dir("binance", "bingx")).unwrap_err();
    assert!(why.contains("binance не взаимодействует с bingx"), "{why}");
    // Пустой список — исключений нет.
    venue_set(&s, cex_link::DENY, "binance", Some(json!([]))).await;
    fill(&s, &mut c);
    assert_eq!(pairs(&c), all);
}

/// (2) venue.cex_cex = нет — в обе стороны; с DEX — торгует.
#[tokio::test]
async fn off_blocks_both_ways() {
    let (core, s) = start("off").await;
    kinds(&core);
    let rec = Rec::load("cex248-off").await;
    let mut c = entry(&rec, &["bingx", "mexc"]);
    venue_set(&s, "venue.cex_cex", "mexc", Some(json!(false))).await;
    fill(&s, &mut c);
    let got = pairs(&c);
    assert!(
        !has(&got, "bingx", "mexc") && !has(&got, "mexc", "bingx"),
        "{got:?}"
    );
    assert!(
        has(&got, "mexc", "bsc_dex") && has(&got, "bsc_dex", "mexc"),
        "{got:?}"
    );
    assert!(cex_link::direction_ok(&core, &dir("bingx", "mexc")).is_err());
    assert!(cex_link::direction_ok(&core, &dir("mexc", "bingx")).is_err());
    assert!(cex_link::direction_ok(&core, &dir("mexc", "bsc_dex")).is_ok());
}

/// Сделка bingx → bsc_dex: монета на кошельке DEX (кейс Тиграна 10.11).
fn deal() -> Deal {
    let mut x = Deal::new(
        "248A".into(),
        dir("bingx", "bsc_dex"),
        json!(null),
        Decimal::ZERO,
    );
    x.at = v("bsc_dex");
    x.qty = Decimal::from(10);
    x.invested = Decimal::from(10);
    x.step = Step::Stuck;
    x.dex_account = Some(DexAccountId("w1".into()));
    x
}

/// (3)–(5) Цели перегона (автоматика и кнопка) по правилам решения 248.
#[tokio::test]
async fn relay_targets_follow_rules() {
    let (core, s) = start("relay").await;
    let rec = Rec::load("cex248-relay").await;
    let c = entry(&rec, &["bingx", "mexc", "binance"]);
    for id in ["bingx", "mexc", "binance", "bsc_dex"] {
        venue_set(&s, "venue.enabled", id, Some(json!(true))).await;
    }
    let x = deal();
    // Виды и справочник — прямо перед расчётом целей, без await между ними (пересборка не вклинится).
    let both = |x: &Deal| {
        kinds(&core);
        core.inner.coins.write().unwrap().live = std::sync::Arc::new(vec![c.clone()]);
        let auto = arb::core::relay::targets(&core, x);
        let (manual, skip) = arb::core::manual_relay_targets::targets(&core, x);
        (auto, manual, skip)
    };
    // (5) Не задано — все биржи цели, как до решения.
    let (auto, manual, skip) = both(&x);
    assert_eq!(auto, vec![v("bingx"), v("mexc"), v("binance")]);
    assert_eq!(manual, auto, "{skip:?}");
    // (3) Кейс Тиграна: у mexc venue.cex_cex = нет — mexc не цель ни автоматики, ни кнопки.
    venue_set(&s, "venue.cex_cex", "mexc", Some(json!(false))).await;
    let (auto, manual, skip) = both(&x);
    assert_eq!(auto, vec![v("bingx"), v("binance")]);
    assert_eq!(manual, auto);
    assert!(
        skip.iter()
            .any(|t| t == "mexc: биржа ↔ биржа выключена у mexc (venue.cex_cex = нет)"),
        "{skip:?}"
    );
    // У биржи покупки «нет» — сделка не уходит ни на одну биржу (кроме возврата на саму bingx).
    venue_set(&s, "venue.cex_cex", "mexc", None).await;
    venue_set(&s, "venue.cex_cex", "bingx", Some(json!(false))).await;
    let (auto, manual, _) = both(&x);
    assert_eq!(auto, vec![v("bingx")]);
    assert_eq!(manual, auto);
    venue_set(&s, "venue.cex_cex", "bingx", None).await;
    // (4) Список биржи покупки: bingx [mexc] — mexc не цель.
    venue_set(&s, cex_link::DENY, "bingx", Some(json!(["mexc"]))).await;
    let (auto, manual, skip) = both(&x);
    assert_eq!(auto, vec![v("bingx"), v("binance")]);
    assert_eq!(manual, auto);
    assert!(
        skip.iter()
            .any(|t| t.starts_with("mexc: bingx не взаимодействует с mexc")),
        "{skip:?}"
    );
    // Список биржи-цели на сделку, начатую на bingx, не влияет: mexc [bingx] — mexc снова цель.
    venue_set(&s, cex_link::DENY, "bingx", None).await;
    venue_set(&s, cex_link::DENY, "mexc", Some(json!(["bingx"]))).await;
    let (auto, manual, _) = both(&x);
    assert_eq!(auto, vec![v("bingx"), v("mexc"), v("binance")]);
    assert_eq!(manual, auto);
}

/// Правило 1 по пути монеты: монета прошла через mexc (venue.cex_cex = нет) — дальше на биржи нельзя, на DEX можно.
#[tokio::test]
async fn path_exchange_off_blocks_relay() {
    let (core, s) = start("path").await;
    kinds(&core);
    let mut x = deal();
    x.direction = dir("bsc_dex", "mexc");
    x.at = v("mexc");
    x.dex_account = None;
    venue_set(&s, "venue.cex_cex", "mexc", Some(json!(false))).await;
    kinds(&core);
    assert!(cex_link::relay_block(&core, &x, &v("bingx")).is_some());
    assert!(cex_link::relay_block(&core, &x, &v("bsc_dex")).is_none());
    // Монета ни на одной бирже не была (сделка начата на DEX, монета на DEX) — DEX → биржа не затрагивается.
    x.at = v("bsc_dex");
    venue_set(&s, "venue.cex_cex", "bingx", Some(json!(false))).await;
    kinds(&core);
    assert!(cex_link::relay_block(&core, &x, &v("bingx")).is_none());
}

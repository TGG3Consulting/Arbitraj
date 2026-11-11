//! Файл: tests/core_daily_venue.rs
//! Что делает: решение 173 (дневной убыток и стоп по бирже) на настоящем ядре и SQLite во временной папке (без площадок
//! в реестре шлюза, моков нет; виды площадок — в inner.kinds, как их кладёт Core::start из реестра).
//! Живой случай 05.11.2026: застряли CREPE1 (MEXC), ION (KuCoin), LUCIC (Gate) — их оценка положила итог дня ниже
//! лимита, и встала ВСЯ торговля, хотя BingX и DEX работали в плюс. Проверяется: (1) убыток MEXC выше лимита — пауза
//! только MEXC, направление BingX ↔ DEX открывается; (2) сделка CEX ↔ CEX считается бирже покупки (дополнение t91;
//! было — обеим биржам), общий итог — без двойного счёта; (3) «Сбросить лимит дня» по площадке снимает её паузу, другие не трогает; (4) общий «Стоп» —
//! только при заданной risk.max_daily_loss_total_usd; (5) перезапуск сохраняет разбивку; старая запись итога дня
//! (без разбивки) разносится по площадкам сделок без потери общего итога; (6) убыток DEX ↔ DEX выше лимита — пауза
//! только циклов DEX ↔ DEX, BingX → DEX и MEXC → DEX открываются.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core, arb::settings, arb::storage, arb::notify, arb::secrets.

use std::path::PathBuf;

use arb::core::Core;
use arb::core::deal::{Deal, Step};
use arb::core::deps::Deps;
use arb::core::risk::{Mode, can_open};
use arb::core::types::{Asset, Direction, VenueId};
use arb::gateway::contract::VenueKind;
use arb::gateway::registry::Registry;
use arb::notify::dispatch::Notifier;
use arb::settings::store::Store;
use arb::settings::value::Scope;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use rust_decimal::Decimal;
use serde_json::json;
use tokio::sync::broadcast;

/// Папка теста (новая).
fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("arb-venue-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Площадка по id.
fn v(id: &str) -> VenueId {
    VenueId(id.into())
}

/// Задаёт настройку. Ошибок нет.
async fn set(s: &Store, name: &str, scope: Scope, val: &str) {
    s.set(name, scope, Some(json!(val)), "тест").await.unwrap();
}

/// Ядро на базе в папке: виды площадок, все обязательные money.* и risk.*, лимит дня площадки 15 («вся система»),
/// режим «В работе». Возвращает: ядро и хранилище настроек.
async fn start(d: &std::path::Path) -> (Core, Store) {
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
        data_dir: d.to_path_buf(),
    })
    .await;
    *core.inner.kinds.write().unwrap() = vec![
        (v("bingx"), VenueKind::Cex),
        (v("bsc_dex"), VenueKind::Dex),
        (v("mexc"), VenueKind::Cex),
    ];
    for (n, x) in [
        ("money.min_profit_usd", "0.1"),
        ("money.min_profit_pct", "0.1"),
        ("money.max_per_trade_usd", "50"),
        ("money.max_per_coin_usd", "100"),
        ("money.max_in_transit_usd", "200"),
        ("risk.max_daily_loss_usd", "15"),
        ("risk.min_bnb_balance", "0.01"),
    ] {
        set(&settings, n, Scope::Global, x).await;
    }
    for id in ["bingx", "bsc_dex", "mexc"] {
        let scope = Scope::Venue(id.into());
        settings
            .set("risk.max_errors_in_row", scope, Some(json!(3)), "тест")
            .await
            .unwrap();
    }
    core.set_mode(Mode::Running).await;
    (core, settings)
}

/// Направление на монете.
fn dir_of(buy: &str, sell: &str) -> Direction {
    Direction {
        asset: Asset {
            ticker: "CAKE".into(),
            bsc_contract: None,
        },
        buy_on: v(buy),
        sell_on: v(sell),
    }
}

/// Закрытая сделка с итогом `r` (факт). Ошибок нет.
async fn closed(core: &Core, id: &str, buy: &str, sell: &str, r: i64) {
    let mut x = Deal::new(id.into(), dir_of(buy, sell), json!(null), Decimal::ZERO);
    x.invested = Decimal::from(20);
    x.proceeds = Decimal::from(20 + r);
    arb::core::ledger::finish(core, &mut x, Step::Closed, "продано").await;
}

/// Итог дня площадки.
fn venue_result(core: &Core, id: &str) -> Decimal {
    core.control().day.venues[id].result_usd
}

#[tokio::test]
async fn mexc_loss_pauses_only_mexc() {
    let (core, _) = start(&dir("only")).await;
    closed(&core, "b1", "bingx", "bsc_dex", 4).await;
    closed(&core, "m1", "mexc", "bsc_dex", -10).await;
    closed(&core, "m2", "mexc", "bsc_dex", -6).await;
    let r = core.control();
    assert_eq!(r.mode, Mode::Running);
    assert!(!r.daily_stop);
    assert!(
        r.venue_paused[&v("mexc")]
            .starts_with("MEXC: убыток за день 16.00 USDT больше лимита 15.00 USDT")
    );
    assert!(!r.venue_paused.contains_key(&v("bingx")));
    // DEX в сделках CEX ↔ DEX не учитывается — его итог ноль, паузы нет.
    assert!(!r.venue_paused.contains_key(&v("bsc_dex")));
    assert_eq!(r.day.venues["bsc_dex"].result_usd, Decimal::ZERO);
    assert_eq!(r.day.venues["bingx"].result_usd, Decimal::from(4));
    assert_eq!(r.day.result_usd, Decimal::from(-12));
    // BingX ↔ DEX торгуется, MEXC — нет.
    assert_eq!(can_open(&core, &dir_of("bingx", "bsc_dex")), Ok(()));
    assert!(
        can_open(&core, &dir_of("mexc", "bsc_dex"))
            .unwrap_err()
            .contains("на паузе")
    );
    // Плюсовая сделка другой площадки паузу MEXC не трогает и новую не ставит.
    closed(&core, "b2", "bingx", "bsc_dex", 1).await;
    assert!(core.control().venue_paused.contains_key(&v("mexc")));
    // «Снять паузу» без сброса: MEXC торгует, пока убыток не вырастет; новый минус — снова пауза.
    core.resume_venue(&v("mexc")).await;
    closed(&core, "b3", "bingx", "bsc_dex", 1).await;
    assert!(!core.control().venue_paused.contains_key(&v("mexc")));
    closed(&core, "m3", "mexc", "bsc_dex", -1).await;
    assert!(core.control().venue_paused.contains_key(&v("mexc")));
}

#[tokio::test]
async fn cex_cex_counts_for_buy_venue_and_total_once() {
    let (core, _) = start(&dir("cexcex")).await;
    closed(&core, "x1", "bingx", "mexc", -3).await;
    let r = core.control();
    assert_eq!(venue_result(&core, "bingx"), Decimal::from(-3));
    // t91: биржа ↔ биржа — только бирже покупки (было −3 и у MEXC).
    assert_eq!(venue_result(&core, "mexc"), Decimal::ZERO);
    assert_eq!(r.day.venues["bsc_dex"].result_usd, Decimal::ZERO);
    assert_eq!(r.day.result_usd, Decimal::from(-3));
    // Цикл DEX ↔ DEX — к DEX.
    closed(&core, "c1", "bsc_dex", "bsc_dex", 2).await;
    assert_eq!(venue_result(&core, "bsc_dex"), Decimal::from(2));
    assert_eq!(core.control().day.result_usd, Decimal::from(-1));
}

#[tokio::test]
async fn reset_by_venue_lifts_only_its_pause() {
    let (core, settings) = start(&dir("reset")).await;
    // Свой лимит площадки сильнее «вся система».
    set(
        &settings,
        "risk.max_daily_loss_usd",
        Scope::Venue("bingx".into()),
        "5",
    )
    .await;
    closed(&core, "m1", "mexc", "bsc_dex", -16).await;
    closed(&core, "b1", "bingx", "bsc_dex", -5).await;
    let r = core.control();
    assert!(r.venue_paused.contains_key(&v("mexc")));
    assert!(r.venue_paused.contains_key(&v("bingx")));
    assert_eq!(
        r.day.venues["bingx"].limit.as_ref().unwrap().max_usd,
        Decimal::from(5)
    );
    let t = core.reset_venue_daily_limit("mexc", "тест").await;
    assert!(t.contains("Пауза площадки по убытку дня снята"), "{t}");
    let r = core.control();
    assert!(!r.venue_paused.contains_key(&v("mexc")));
    assert!(r.venue_paused.contains_key(&v("bingx")));
    let m = &r.day.venues["mexc"];
    assert_eq!(m.limit.as_ref().unwrap().loss_usd, Decimal::ZERO);
    assert_eq!(m.result_usd, Decimal::from(-16));
    assert_eq!(r.day.result_usd, Decimal::from(-21));
    // Общий сброс — снимает паузы всех площадок; режим не меняется.
    core.reset_daily_limit("тест").await;
    let r = core.control();
    assert!(r.venue_paused.is_empty());
    assert_eq!(r.mode, Mode::Running);
}

#[tokio::test]
async fn total_stop_only_when_total_set() {
    let (core, settings) = start(&dir("total")).await;
    closed(&core, "m1", "mexc", "bsc_dex", -16).await;
    closed(&core, "x1", "bingx", "bsc_dex", -16).await;
    let r = core.control();
    assert_eq!(r.mode, Mode::Running);
    assert!(r.day.limit.is_none());
    assert_eq!(r.venue_paused.len(), 2);
    set(
        &settings,
        "risk.max_daily_loss_total_usd",
        Scope::Global,
        "30",
    )
    .await;
    closed(&core, "d1", "bsc_dex", "bsc_dex", -1).await;
    let r = core.control();
    assert_eq!(r.mode, Mode::Stopped);
    assert!(r.daily_stop);
    assert!(
        r.reason
            .unwrap()
            .starts_with("вся система: убыток за день 33.00 USDT")
    );
}

#[tokio::test]
async fn restart_keeps_split_and_old_record_is_split() {
    let d0 = dir("restart");
    {
        let (core, _) = start(&d0).await;
        closed(&core, "x1", "bingx", "mexc", -3).await;
        closed(&core, "m1", "mexc", "bsc_dex", -13).await;
        core.reset_venue_daily_limit("bingx", "тест").await;
    }
    {
        let (core, _) = start(&d0).await;
        let r = core.control();
        assert_eq!(r.day.result_usd, Decimal::from(-16));
        // t91: BingX → MEXC (−3) — только бирже покупки BingX (было −16 у MEXC и пауза: 16 ≥ 15).
        assert_eq!(venue_result(&core, "mexc"), Decimal::from(-13));
        assert_eq!(venue_result(&core, "bingx"), Decimal::from(-3));
        assert!(r.day.venues["bingx"].limit_reset.is_some());
        // Убыток MEXC 13 < 15 — паузы нет.
        assert!(!r.venue_paused.contains_key(&v("mexc")));
    }
    // Старая запись (до решения 173): только общий итог — разносится по площадкам сделок дня.
    let db = Db::open(&d0.join("t.sqlite")).unwrap();
    let today = chrono::Utc::now().date_naive().to_string();
    let body = json!({"final_usd": "-16", "provisional": {}}).to_string();
    db.call(move |c| {
        c.execute(
            "UPDATE daily_result SET body = ?2 WHERE day = ?1",
            rusqlite::params![today, body],
        )
        .map(|_| ())
    })
    .await
    .unwrap();
    let (core, _) = start(&d0).await;
    let r = core.control();
    assert_eq!(r.day.result_usd, Decimal::from(-16));
    // t91: разнесение — по нынешнему правилу (было −16 у MEXC).
    assert_eq!(venue_result(&core, "mexc"), Decimal::from(-13));
    assert_eq!(venue_result(&core, "bingx"), Decimal::from(-3));
}

#[tokio::test]
async fn dex_loss_pauses_only_dex_cycles() {
    let (core, _) = start(&dir("cycles")).await;
    closed(&core, "c1", "bsc_dex", "bsc_dex", -16).await;
    let r = core.control();
    // Площадка DEX целиком не на паузе — только циклы DEX ↔ DEX.
    assert!(!r.venue_paused.contains_key(&v("bsc_dex")));
    assert!(
        r.cycle_paused["bsc_dex"]
            .starts_with("DEX ↔ DEX: убыток за день 16.00 USDT больше лимита 15.00 USDT")
    );
    assert!(r.day.venues["bsc_dex"].paused);
    let why = can_open(&core, &dir_of("bsc_dex", "bsc_dex")).unwrap_err();
    assert!(why.contains("циклы DEX ↔ DEX на паузе"), "{why}");
    assert_eq!(can_open(&core, &dir_of("bingx", "bsc_dex")), Ok(()));
    assert_eq!(can_open(&core, &dir_of("mexc", "bsc_dex")), Ok(()));
    // Сброс лимита DEX снимает паузу циклов.
    core.reset_venue_daily_limit("bsc_dex", "тест").await;
    assert!(core.control().cycle_paused.is_empty());
    assert_eq!(can_open(&core, &dir_of("bsc_dex", "bsc_dex")), Ok(()));
    // «Снять паузу» DEX тоже снимает паузу циклов.
    closed(&core, "c2", "bsc_dex", "bsc_dex", -16).await;
    assert!(!core.control().cycle_paused.is_empty());
    core.resume_venue(&v("bsc_dex")).await;
    assert!(core.control().cycle_paused.is_empty());
}

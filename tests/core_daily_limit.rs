//! Файл: tests/core_daily_limit.rs
//! Что делает: решения 152 и 153 на настоящей SQLite во временной папке (ядро без площадок в реестре, моков нет).
//! Живой случай 04.11.2026, сделка 20261104173851521BABYDOGE (mexc → bsc_dex): куплено 139 362 031 588 BABYDOGE на MEXC
//! за 45 USDT (+ комиссия 0.036 — так в журнале: «предварительный 45.036»), вывод отказал (MEXC 16021) → Halted;
//! «Продать сейчас» → продано за 44.86 (комиссия 0.036) → итог −0.212; итог дня до сделки +32.088 (из журнала:
//! «убыток за день 12.948 … из них предварительный 45.036»), лимит 6. Числа — из журнала живой системы; деление 45.036
//! на 45 + 0.036 и стоимость монет 44.824 (= выручка продажи через 26 с минус её комиссия) восстановлены по ним.
//! Проверяется: (1) оценка позиции по рынку — −0.212, а не −45.036; лимит не срабатывает; факт продажи — в ту же
//! сделку, оценка снимается ровно на неё; (2) цены нет — худший случай с пометкой, стоп по лимиту; после факта —
//! «лимит сейчас не превышен», режим остаётся «Стоп»; (3) «Сбросить лимит дня»: итог дня и сделки не меняются, лимит
//! считает только после отметки, отметка переживает перезапуск, режим сам не меняется; вчерашняя отметка не действует.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core, arb::settings, arb::storage, arb::notify, arb::secrets.

use std::path::PathBuf;
use std::str::FromStr;

use arb::core::Core;
use arb::core::daily_value::estimate;
use arb::core::deal::{Deal, Step};
use arb::core::deps::Deps;
use arb::core::risk::Mode;
use arb::core::types::{Asset, Direction, VenueId};
use arb::gateway::registry::Registry;
use arb::notify::dispatch::Notifier;
use arb::settings::store::Store;
use arb::settings::value::Scope;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use rust_decimal::Decimal;
use serde_json::json;
use tokio::sync::broadcast;

/// Число из строки (суммы живого случая).
fn d(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

/// Папка теста (новая).
fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("arb-core-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Запускает ядро на базе в папке, лимит дня 6 USDT (как в живом случае), режим «В работе».
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
    settings
        .set(
            "risk.max_daily_loss_usd",
            Scope::Global,
            Some(json!("6")),
            "тест",
        )
        .await
        .unwrap();
    core.set_mode(Mode::Running).await;
    (core, settings)
}

/// Сделка направления mexc → bsc_dex на монете (монеты на MEXC).
fn deal(id: &str, ticker: &str) -> Deal {
    let dir = Direction {
        asset: Asset {
            ticker: ticker.into(),
            bsc_contract: None,
        },
        buy_on: VenueId("mexc".into()),
        sell_on: VenueId("bsc_dex".into()),
    };
    Deal::new(id.into(), dir, json!(null), Decimal::ZERO)
}

/// Закрытая сделка дня с итогом `r` (факт). Ошибок нет.
async fn closed(core: &Core, id: &str, r: Decimal) {
    let mut x = deal(id, "CAKE");
    x.invested = Decimal::from(20);
    x.proceeds = Decimal::from(20) + r;
    arb::core::ledger::finish(core, &mut x, Step::Closed, "продано").await;
}

/// BABYDOGE сразу после покупки на MEXC (до отказа вывода).
fn babydoge() -> Deal {
    let mut x = deal("20261104173851521BABYDOGE", "BABYDOGE");
    x.qty = d("139362031588");
    x.invested = d("45");
    x.spent = d("0.036");
    x.step = Step::Halted;
    x
}

/// Записи журнала вида `kind`. Ошибок нет.
async fn journal(core: &Core, kind: &str) -> Vec<String> {
    let all = core.inner.deps.journal.list(500, None).await.unwrap();
    all.into_iter()
        .filter(|e| e.kind == kind)
        .map(|e| e.message)
        .collect()
}

#[tokio::test]
async fn babydoge_estimate_by_market_not_whole_order() {
    let (core, _) = start(&dir("limit-babydoge")).await;
    closed(&core, "day", d("32.088")).await;
    let mut x = babydoge();
    // Монеты лежат на MEXC и стоят по рынку 44.824 USDT чистыми.
    let est = estimate(&x, Ok(d("44.824")));
    assert!(est.priced);
    arb::core::daily::record(&core, &mut x, est).await;
    let day = core.control().day;
    assert_eq!(day.provisional_usd, d("-0.212"));
    assert_ne!(day.provisional_usd, d("-45.036"));
    assert_eq!(day.result_usd, d("31.876"));
    assert!(day.unpriced.is_empty());
    // Позиция стоит ≈ вложенному — лимит не срабатывает.
    assert_eq!(core.control().mode, Mode::Running);
    assert!(!day.limit.unwrap().exceeded);
    // «Продать сейчас»: факт продажи пишется в ту же сделку (fills), итог = выручка − вложено − затраты.
    x.proceeds = d("44.86");
    x.spent += d("0.036");
    x.qty = Decimal::ZERO;
    arb::core::ledger::finish(
        &core,
        &mut x,
        Step::Closed,
        "продано (кнопка «Продать сейчас»)",
    )
    .await;
    assert_eq!(x.result_usd, Some(d("-0.212")));
    let day = core.control().day;
    assert_eq!(day.result_usd, d("31.876"));
    assert_eq!(day.final_usd, d("31.876"));
    assert!(day.provisional.is_empty());
}

#[tokio::test]
async fn no_price_worst_case_then_fact_shows_limit_not_exceeded() {
    let (core, _) = start(&dir("limit-noprice")).await;
    closed(&core, "day", d("32.088")).await;
    let mut x = babydoge();
    // Площадки в реестре нет — цены нет: худший случай с пометкой (как живой стоп 17:38:53).
    arb::core::ledger::finish(&core, &mut x, Step::Halted, "вывод отклонён: MEXC 16021").await;
    let r = core.control();
    assert_eq!(r.day.provisional_usd, d("-45.036"));
    assert!(r.day.unpriced.contains(&x.id));
    assert!(x.timeline.iter().any(|(_, t)| t.contains("цены нет")));
    assert_eq!(r.mode, Mode::Stopped);
    assert!(r.daily_stop);
    assert_eq!(r.day.limit.as_ref().unwrap().loss_usd, d("12.948"));
    // Факт пришёл — лимит не превышен, но остановка сама не снимается; в журнале — подсказка.
    x.proceeds = d("44.86");
    x.spent += d("0.036");
    x.qty = Decimal::ZERO;
    arb::core::ledger::finish(
        &core,
        &mut x,
        Step::Closed,
        "продано (кнопка «Продать сейчас»)",
    )
    .await;
    let r = core.control();
    assert_eq!(x.result_usd, Some(d("-0.212")));
    assert_eq!(r.day.result_usd, d("31.876"));
    assert!(r.day.unpriced.is_empty());
    assert_eq!(r.mode, Mode::Stopped);
    assert!(r.daily_stop);
    assert!(!r.day.limit.unwrap().exceeded);
    let j = journal(&core, "core.daily_loss").await;
    assert!(j.iter().any(|m| m.contains("Лимит дня сейчас не превышен")));
    // «Старт» — решение человека: отметка «остановлена по лимиту» снимается.
    core.set_mode(Mode::Running).await;
    assert!(!core.control().daily_stop);
}

#[tokio::test]
async fn reset_counts_only_after_mark_and_survives_restart() {
    let d0 = dir("limit-reset");
    {
        let (core, _) = start(&d0).await;
        closed(&core, "a", d("-5")).await;
        closed(&core, "b", d("-2")).await;
        assert_eq!(core.control().mode, Mode::Stopped);
        let text = core.reset_daily_limit("тест").await;
        assert!(text.contains("остаётся в «Стоп»"));
        let r = core.control();
        // Итог дня и сделки не меняются; система сама не запускается.
        assert_eq!(r.day.result_usd, d("-7"));
        assert_eq!(core.deals().len(), 2);
        assert_eq!(r.mode, Mode::Stopped);
        assert_eq!(r.day.limit_reset.as_ref().unwrap().base_usd, d("-7"));
        assert_eq!(r.day.limit.as_ref().unwrap().loss_usd, Decimal::ZERO);
        assert_eq!(journal(&core, "core.loss_limit_reset").await.len(), 1);
        core.set_mode(Mode::Running).await;
        closed(&core, "c", d("-3")).await;
        let r = core.control();
        assert_eq!(r.mode, Mode::Running);
        assert_eq!(r.day.limit.unwrap().loss_usd, d("3"));
        assert_eq!(r.day.result_usd, d("-10"));
    }
    // Перезапуск: отметка сброса восстановлена, лимит считает после неё.
    let (core, _) = start(&d0).await;
    let r = core.control();
    assert_eq!(r.day.result_usd, d("-10"));
    assert_eq!(r.day.limit.unwrap().loss_usd, d("3"));
    closed(&core, "e", d("-3")).await;
    assert_eq!(core.control().mode, Mode::Stopped);
}

#[tokio::test]
async fn yesterday_reset_has_no_force() {
    let d0 = dir("limit-yday");
    {
        let db = Db::open(&d0.join("t.sqlite")).unwrap();
        let y = chrono::Utc::now().date_naive() - chrono::Duration::days(1);
        let at = format!("{y}T17:45:00Z");
        let body = json!({"final_usd": "-7", "provisional": {},
            "limit_reset": {"at": at, "base_usd": "-7", "by": "тест"}})
        .to_string();
        let day = y.to_string();
        db.call(move |c| {
            c.execute(
                "INSERT INTO daily_result (day, body, updated_at) VALUES (?1, ?2, ?3)",
                rusqlite::params![day, body, "t"],
            )
            .map(|_| ())
        })
        .await
        .unwrap();
    }
    let (core, _) = start(&d0).await;
    let r = core.control();
    assert_eq!(r.day.result_usd, Decimal::ZERO);
    assert!(r.day.limit_reset.is_none());
    closed(&core, "a", d("-6")).await;
    assert_eq!(core.control().mode, Mode::Stopped);
}

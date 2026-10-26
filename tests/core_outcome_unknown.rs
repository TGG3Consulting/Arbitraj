//! Файл: tests/core_outcome_unknown.rs
//! Что делает: «исход неизвестен» (OutcomeUnknown — транзакция могла исполниться) в ядре на настоящей SQLite: даже при
//! правиле шлюза «повторить» операция не повторяется, площадка ставится на паузу один раз (повторная такая же ошибка
//! — без новой паузы и уведомлений); код ошибки шлюза доходит до исполнителя (OpError) — по нему сделка
//! останавливается до ручной проверки, без застрявшей монеты и второго перевода (core/executor.rs).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core, arb::gateway, arb::storage, arb::settings, arb::notify. Моков нет: площадок в реестре нет,
//! проверяется решение ядра по ошибке шлюза.

use arb::core::Core;
use arb::core::deps::Deps;
use arb::core::op_error::OpError;
use arb::core::rules_apply::{self, Next};
use arb::core::types::VenueId;
use arb::gateway::errors::{ErrorCode, GatewayError};
use arb::gateway::registry::Registry;
use arb::gateway::rules::{Rule, RuleAction, RuleScope};
use arb::notify::dispatch::Notifier;
use arb::settings::store::Store;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use tokio::sync::broadcast;

/// Ядро на базе во временной папке.
async fn start(name: &str) -> (Core, Db) {
    let d = std::env::temp_dir().join(format!("arb-core-{name}-{}", std::process::id()));
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
        settings,
        journal,
        notifier,
        db: db.clone(),
        changes,
        buffer: 64,
        data_dir: d.to_path_buf(),
    })
    .await;
    (core, db)
}

#[tokio::test]
async fn outcome_unknown_never_retried_and_pauses_once() {
    let (core, db) = start("unknown").await;
    let v = VenueId("bsc_dex".into());
    // Правило «повторять» на этот код — ядро всё равно не повторяет.
    let rule = Rule {
        id: 0,
        event: ErrorCode::OutcomeUnknown,
        condition: None,
        action: RuleAction::Retry {
            attempts: 5,
            pause_ms: 1,
        },
        scope: RuleScope::All,
        enabled: true,
    };
    arb::storage::rules::create(&db, rule).await.unwrap();
    let e = GatewayError::new(
        ErrorCode::OutcomeUnknown,
        Some(v.clone()),
        "nonce занят, квитанции нет",
    );
    assert_eq!(
        rules_apply::on_error(&core, "Вывод", &v, Some("d1"), &e, 1).await,
        Next::Stop
    );
    let first = core
        .control()
        .venue_paused
        .get(&v)
        .cloned()
        .expect("площадка на паузе");
    assert_eq!(
        rules_apply::on_error(&core, "Вывод", &v, Some("d1"), &e, 2).await,
        Next::Stop
    );
    assert_eq!(
        core.control().venue_paused.get(&v),
        Some(&first),
        "пауза одна, причина не меняется"
    );
    // Код доходит до исполнителя.
    let op = OpError::from(&e);
    assert!(op.outcome_unknown());
    assert!(!OpError::from("обычный отказ".to_string()).outcome_unknown());
}

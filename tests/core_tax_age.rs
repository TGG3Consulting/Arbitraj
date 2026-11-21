//! Файл: tests/core_tax_age.rs
//! Что делает: решение 234, п. 3 — свежесть замера налогов (core/tax_age.rs): замер старше tax.max_age_s — возраст в
//! секундах (отказ), ровно на пределе и моложе — свежий, замера нет — решает адаптер (None); на настоящем ядре и SQLite:
//! tax.max_age_s не задана или DEX-сторон в реестре нет — проверка не мешает.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core::tax_age, tests/size_common (направление CAKE), arb::settings, arb::storage. Моков нет.

mod size_common;

use arb::core::Core;
use arb::core::deps::Deps;
use arb::core::tax_age::{check, stale};
use arb::gateway::registry::Registry;
use arb::notify::dispatch::Notifier;
use arb::settings::store::Store;
use arb::settings::value::Scope;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use chrono::{Duration, TimeZone, Utc};
use serde_json::json;
use tokio::sync::broadcast;

#[test]
fn stale_by_age() {
    let now = Utc.with_ymd_and_hms(2026, 11, 8, 17, 50, 20).unwrap();
    let at = |s: i64| Some(now - Duration::seconds(s));
    assert_eq!(stale(at(7201), now, 7200), Some(7201));
    assert_eq!(stale(at(7200), now, 7200), None);
    assert_eq!(stale(at(10), now, 7200), None);
    assert_eq!(stale(None, now, 7200), None);
}

/// Ядро на базе во временной папке (как tests/core_dry_mode.rs).
async fn start(name: &str) -> (Core, Store) {
    let d = std::env::temp_dir().join(format!("arb-taxage-{name}-{}", std::process::id()));
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

#[tokio::test]
async fn unset_or_no_dex_passes() {
    let (core, s) = start("unset").await;
    let dir = size_common::calc::direction();
    assert!(
        check(&core, &dir, Utc::now()).is_ok(),
        "не задана — проверки нет"
    );
    s.set("tax.max_age_s", Scope::Global, Some(json!(7200)), "тест")
        .await
        .unwrap();
    assert!(
        check(&core, &dir, Utc::now()).is_ok(),
        "DEX-сторон в реестре нет"
    );
}

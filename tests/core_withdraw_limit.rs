//! Файл: tests/core_withdraw_limit.rs
//! Что проверяет: решение 180 — лимит выводов по бирже (core/withdraw_limit.rs) на настоящем ядре:
//! настроек нет — как раньше (места не нужны); окно занято — второй сигнал пропускается с причиной; сделка закончилась
//! без вывода — место освобождается; вывод без места ждёт освобождения окна; ключ «по монете» не мешает другой монете.
//! Значения — живой лимит Gate «Withdrawal frequency is limited to 10s» (короче для теста: 1 вывод за 300 мс).
//! Кто вызывает: cargo test. От чего зависит: core (Core, withdraw_limit), settings, storage.

use std::time::{Duration, Instant};

use arb::core::Core;
use arb::core::deps::Deps;
use arb::core::types::VenueId;
use arb::core::withdraw_limit as wl;
use arb::gateway::registry::Registry;
use arb::notify::dispatch::Notifier;
use arb::settings::store::Store;
use arb::settings::value::Scope;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use serde_json::json;
use tokio::sync::broadcast;

/// Ядро; лимит у gate: окно ms, max, scope (None — не задавать).
async fn start(name: &str, lim: Option<(u64, u64, &str)>) -> Core {
    let d = std::env::temp_dir().join(format!("arb-180-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let db = Db::open(&d.join("t.sqlite")).unwrap();
    let (changes, _) = broadcast::channel(64);
    let (jtx, _) = broadcast::channel(64);
    let journal = Journal::new(db.clone(), jtx);
    let settings = Store::load(db.clone(), changes.clone(), journal.clone())
        .await
        .unwrap();
    if let Some((w, m, s)) = lim {
        let g = || Scope::Venue("gate".into());
        for (n, v) in [
            ("transfer.withdraw_window_ms", json!(w)),
            ("transfer.withdraw_max_in_window", json!(m)),
            ("transfer.withdraw_limit_scope", json!(s)),
        ] {
            settings.set(n, g(), Some(v), "тест").await.unwrap();
        }
    }
    let notifier = Notifier {
        http: reqwest::Client::new(),
        settings: settings.clone(),
        vault: arb::secrets::Vault::locked(d.join("secrets.enc")),
        journal: journal.clone(),
    };
    Core::start(Deps {
        gw: Registry::new(),
        settings,
        journal,
        notifier,
        db,
        changes,
        buffer: 64,
        data_dir: d.to_path_buf(),
    })
    .await
}

fn gate() -> VenueId {
    VenueId("gate".into())
}

#[tokio::test]
async fn no_settings_works_as_before() {
    let c = start("none", None).await;
    for _ in 0..5 {
        assert!(wl::reserve(&c, &gate(), "ACH").unwrap().is_none());
    }
    let t = Instant::now();
    wl::take(&c, "", &gate(), "USDT").await;
    assert!(t.elapsed() < Duration::from_millis(50));
}

#[tokio::test]
async fn busy_window_skips_signal_and_frees_after_deal_without_withdraw() {
    let c = start("busy", Some((300, 1, "venue"))).await;
    let s = wl::reserve(&c, &gate(), "ACH").unwrap().unwrap();
    wl::bind(&c, "d1", s);
    let e = wl::reserve(&c, &gate(), "LUCIC").err().unwrap();
    assert!(e.contains("лимит выводов gate: 1 за 300 мс"), "{e}");
    // Сделка закончилась до вывода — место свободно сразу.
    wl::release(&c, "d1");
    assert!(wl::reserve(&c, &gate(), "LUCIC").unwrap().is_some());
}

#[tokio::test]
async fn withdraw_uses_deal_slot_and_return_waits_for_window() {
    let c = start("wait", Some((300, 1, "venue"))).await;
    let s = wl::reserve(&c, &gate(), "ACH").unwrap().unwrap();
    wl::bind(&c, "d1", s);
    // Вывод сделки — по её месту, без ожидания.
    let t = Instant::now();
    wl::take(&c, "d1", &gate(), "ACH").await;
    assert!(t.elapsed() < Duration::from_millis(50));
    // Место использовано — конец сделки его не освобождает (вывод был).
    wl::release(&c, "d1");
    assert!(wl::reserve(&c, &gate(), "X").is_err());
    // Возврат USDT без места — ждёт освобождения окна (~300 мс), не пропускается.
    let t = Instant::now();
    wl::take(&c, "", &gate(), "USDT").await;
    assert!(
        t.elapsed() >= Duration::from_millis(250),
        "{:?}",
        t.elapsed()
    );
}

#[tokio::test]
async fn per_coin_scope_does_not_block_other_coin() {
    let c = start("coin", Some((300, 1, "coin"))).await;
    let a = wl::reserve(&c, &gate(), "ACH").unwrap().unwrap();
    assert!(wl::reserve(&c, &gate(), "ach").is_err());
    assert!(wl::reserve(&c, &gate(), "LUCIC").unwrap().is_some());
    drop(a);
}

/// Решение 182: ордер биржи снят без исполнения — не «ошибка площадки подряд» (деньги не тронуты).
#[test]
fn order_not_filled_is_not_venue_error() {
    use arb::gateway::errors::{ErrorCode, GatewayError};
    let e = GatewayError::new(
        ErrorCode::OrderNotFilled,
        None,
        "Продажа 155 GENIUS не исполнилась",
    );
    assert!(!arb::core::risk_count::counts(&e));
}

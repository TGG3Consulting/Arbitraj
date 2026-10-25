//! Файл: tests/bsc_dex_live_node.rs
//! Что делает: темп узла HTTP (bsc.node_http_rps — вызовы не залпом, пачка из N — N мест, без настройки запрос не
//! уходит) и одна запись журнала на вид причины (pools/progress.rs). Общей «стартовой загрузки» больше нет —
//! налоги монеты ждут только её пулов (решение 63, tests/bsc_dex_coin_start.rs).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::{node::pace, pools::progress}, arb::settings, arb::storage (настоящая SQLite во
//! временной папке). Сети нет.

use serde_json::json;
use tokio::sync::broadcast;

use arb::gateway::errors::ErrorCode;
use arb::settings::scope::Scope;
use arb::settings::store::Store;
use arb::storage::db::Db;
use arb::storage::journal::{Journal, Level};
use arb::storage::raw_log::RawLog;
use arb::venues::bsc_dex::ctx::Ctx;
use arb::venues::bsc_dex::node::pace;
use arb::venues::bsc_dex::pools::progress::{Signals, reason_key};

/// Окружение на настоящей SQLite.
async fn ctx(name: &str) -> Ctx {
    let d = std::env::temp_dir().join(format!("arb-live-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let db = Db::open(&d.join("t.sqlite")).unwrap();
    let (changes, _) = broadcast::channel(64);
    let (jtx, _) = broadcast::channel(64);
    let journal = Journal::new(db.clone(), jtx);
    let settings = Store::load(db, changes, journal.clone()).await.unwrap();
    Ctx {
        raw: RawLog::start(d.join("raw"), settings.clone()),
        pace: Default::default(),
        failover: Default::default(),
        index: Default::default(),
        settings,
        vault: arb::secrets::Vault::locked(d.join("secrets.enc")),
        http: reqwest::Client::new(),
        journal,
    }
}

#[tokio::test]
async fn node_calls_are_paced_by_setting() {
    let c = ctx("pace").await;
    // Не задан темп — запрос не уходит.
    assert_eq!(
        pace::wait(&c, 1).await.unwrap_err().code,
        ErrorCode::NotConfigured
    );
    c.settings
        .set("bsc.node_http_rps", Scope::Global, Some(json!(100)), "тест")
        .await
        .unwrap();
    let t0 = tokio::time::Instant::now();
    for _ in 0..5 {
        pace::wait(&c, 1).await.unwrap();
    }
    // 5 вызовов при 100 в секунду: последний уходит не раньше 40 мс после первого.
    assert!(t0.elapsed() >= std::time::Duration::from_millis(40));
    // Пачка из 10 вызовов занимает 0,1 с очереди.
    let t1 = tokio::time::Instant::now();
    pace::wait(&c, 10).await.unwrap();
    pace::wait(&c, 1).await.unwrap();
    assert!(t1.elapsed() >= std::time::Duration::from_millis(100));
}

#[tokio::test]
async fn one_journal_record_per_reason() {
    let c = ctx("signals").await;
    let s = Signals::default();
    // Одна запись на вид причины: блоки и числа не делают причину новой.
    for b in [100, 101, 102] {
        s.once(
            &c,
            Level::Warn,
            "venue.ws",
            format!("узел HTTP стоит на блоке {b}"),
        )
        .await;
    }
    let list = c.journal.list(100, None).await.unwrap();
    assert_eq!(list.iter().filter(|e| e.kind == "venue.ws").count(), 1);
    assert_eq!(
        reason_key("расхождение 61.4% > 0.1%"),
        "расхождение .% > .%"
    );
}

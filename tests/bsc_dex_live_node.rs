//! Файл: tests/bsc_dex_live_node.rs
//! Что делает: темп узла HTTP (bsc.node_http_rps — вызовы не залпом, пачка из N — N мест, без настройки запрос не
//! уходит) и сигналы подписки пулов (конец стартовой загрузки — сразу после первого приёма пулов пачки, а не по
//! концу её сессии; одна запись журнала на вид причины).
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
async fn start_signal_and_one_journal_record_per_reason() {
    let c = ctx("signals").await;
    let s = Signals::default();
    s.expect(2);
    s.loaded();
    let waited = tokio::time::timeout(std::time::Duration::from_millis(50), s.wait_start()).await;
    assert!(waited.is_err(), "одна пачка ещё грузится");
    s.loaded();
    tokio::time::timeout(std::time::Duration::from_millis(50), s.wait_start())
        .await
        .expect("стартовая загрузка закончена");
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

#[tokio::test]
async fn listening_batch_releases_start_right_after_first_admit() {
    // Живой запуск 2026-10-25 (main 7626493): все 2853 пула прочитаны, а первый проход налогов не начался —
    // «первая загрузка пачки» отмечалась только по концу её сессии, а здоровая пачка после приёма слушает вечно.
    // Теперь отметка ставится сразу после первого приёма пулов (session.rs) и ровно один раз на пачку.
    let s = Signals::default();
    s.expect(2);
    let mut a = s.first_load(true);
    let mut b = s.first_load(true);
    // Пачка A приняла пулы и продолжает слушать: её сессия не кончилась, но отметка уже поставлена.
    a.mark();
    // Повторная отметка той же пачки (конец сессии, переподключение) не засчитывается за пачку B.
    a.mark();
    let waited = tokio::time::timeout(std::time::Duration::from_millis(50), s.wait_start()).await;
    assert!(waited.is_err(), "пачка B ещё не приняла пулы");
    b.mark();
    tokio::time::timeout(std::time::Duration::from_millis(50), s.wait_start())
        .await
        .expect("обе пачки приняли пулы и слушают — стартовая загрузка закончена");
    // Пачка, найденная позже (не стартовая), стартовую загрузку не трогает.
    let mut late = s.first_load(false);
    late.mark();
}

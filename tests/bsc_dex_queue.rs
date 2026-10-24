//! Файл: tests/bsc_dex_queue.rs
//! Что делает: очередь кошелька DEX переживает перезапуск (SQLite, storage/wallet_queue.rs): «остановлена до ручной
//! проверки» остаётся остановленной; «ждёт итога» при запуске становится «остановлена» (исход неизвестен);
//! снятие вручную — свободна.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::wallet::{nonce, queue_store}, arb::storage (настоящая SQLite во временной папке).

use alloy_primitives::B256;
use chrono::Utc;
use tokio::sync::broadcast;

use arb::core::types::WalletQueueState;
use arb::settings::store::Store;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use arb::storage::raw_log::RawLog;
use arb::venues::bsc_dex::ctx::Ctx;
use arb::venues::bsc_dex::wallet::nonce::Nonces;
use arb::venues::bsc_dex::wallet::queue_store;

/// Окружение на базе в папке (та же папка — «перезапуск»).
async fn ctx(d: &std::path::Path) -> Ctx {
    let db = Db::open(&d.join("t.sqlite")).unwrap();
    let (changes, _) = broadcast::channel(64);
    let (jtx, _) = broadcast::channel(64);
    let journal = Journal::new(db.clone(), jtx);
    let settings = Store::load(db, changes, journal.clone()).await.unwrap();
    Ctx {
        raw: RawLog::start(d.join("raw"), settings.clone()),
        pace: Default::default(),
        failover: Default::default(),
        settings,
        vault: arb::secrets::Vault::locked(d.join("secrets.enc")),
        http: reqwest::Client::new(),
        journal,
    }
}

#[tokio::test]
async fn busy_at_restart_becomes_halted_until_released() {
    let d = std::env::temp_dir().join(format!("arb-dex-queue-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let c = ctx(&d).await;
    let busy = WalletQueueState::Busy {
        nonce: 7,
        hash: B256::repeat_byte(1),
        since: Utc::now(),
    };
    queue_store::save(&c, &busy).await;
    // «Перезапуск»: новое окружение на той же базе, новая очередь.
    let c2 = ctx(&d).await;
    let q = Nonces::default();
    q.restore(&c2).await;
    match q.state() {
        WalletQueueState::Halted { nonce, reason, .. } => {
            assert_eq!(nonce, Some(7));
            assert!(reason.contains("перезапуск"));
        }
        s => panic!("ожидалась остановка, а {s:?}"),
    }
    let free = q.release().unwrap();
    assert_eq!(free, WalletQueueState::Free);
    queue_store::save(&c2, &free).await;
    let q3 = Nonces::default();
    q3.restore(&ctx(&d).await).await;
    assert_eq!(q3.state(), WalletQueueState::Free);
    assert!(q3.release().is_err(), "свободную очередь снимать нечего");
}

//! Файл: tests/kyber_receipt_before_send_398.rs
//! Что делает: решение 398 на настоящем транспорте адаптера KyberSwap (venues/kyberswap/tx.rs send): сроки ожидания
//! квитанции kyber.receipt_poll_ms / kyber.receipt_timeout_ms читаются ДО отправки. Не задана любая — send отказывает
//! NotConfigured с именем настройки, а к узлу BSC (kyber.node_http_url — локальный сервер, считающий подключения) не
//! уходит ни одного запроса: ни eth_gasPrice, ни eth_sendRawTransaction. Раньше транзакция уже уходила, а ожидание
//! квитанции падало «не задана» — итог сделки бот не узнавал (журнал настроек, docs/SETTINGS_CASES.md).
//! Данных площадки тест не использует: отказ случается до первого запроса.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::kyberswap (ctx, tx), arb::settings, arb::storage, arb::secrets, arb::gateway.

use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use alloy_primitives::{Address, Bytes, U256};
use arb::gateway::errors::ErrorCode;
use arb::settings::value::Scope;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use arb::storage::raw_log::RawLog;
use arb::venues::kyberswap::{ctx::Ctx, tx};
use serde_json::json;
use tokio::net::TcpListener;
use tokio::sync::broadcast;

/// Локальный «узел»: только считает подключения. Возвращает: адрес, счётчик.
async fn node() -> (String, Arc<AtomicUsize>) {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    let hits = Arc::new(AtomicUsize::new(0));
    let h = hits.clone();
    tokio::spawn(async move {
        while let Ok((_s, _)) = l.accept().await {
            h.fetch_add(1, Ordering::SeqCst);
        }
    });
    (url, hits)
}

/// Окружение KyberSwap на SQLite во временной папке; задан адрес узла и одна из двух настроек сроков (skip — какую
/// не задавать).
async fn start(skip: &str) -> (Ctx, Arc<AtomicUsize>) {
    let d = std::env::temp_dir().join(format!("arb-kyber-398-{skip}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let db = Db::open(&d.join("t.sqlite")).unwrap();
    let (changes, _) = broadcast::channel(64);
    let (jtx, _) = broadcast::channel(64);
    let journal = Journal::new(db.clone(), jtx);
    let settings = arb::settings::store::Store::load(db, changes, journal.clone())
        .await
        .unwrap();
    let (url, hits) = node().await;
    for (k, v) in [
        ("kyber.node_http_url", json!(url)),
        ("kyber.receipt_poll_ms", json!(100)),
        ("kyber.receipt_timeout_ms", json!(1000)),
    ] {
        if k == skip {
            continue;
        }
        settings
            .set(k, Scope::Global, Some(v), "тест")
            .await
            .unwrap();
    }
    let ctx = Ctx {
        settings: settings.clone(),
        vault: arb::secrets::Vault::locked(d.join("secrets.enc")),
        http: reqwest::Client::new(),
        journal,
        raw: RawLog::start(d.join("raw"), settings),
        pace: Default::default(),
        wallet: None,
    };
    (ctx, hits)
}

#[tokio::test]
async fn no_transaction_without_receipt_settings() {
    for skip in ["kyber.receipt_poll_ms", "kyber.receipt_timeout_ms"] {
        let (ctx, hits) = start(skip).await;
        let next = StdMutex::new(None);
        let sent = tx::send(
            &ctx,
            &next,
            Address::ZERO,
            Address::ZERO,
            Bytes::new(),
            U256::ZERO,
        )
        .await;
        let Err(e) = sent else {
            panic!("без срока квитанции транзакция не отправляется");
        };
        assert_eq!(e.code, ErrorCode::NotConfigured, "{}", e.message);
        assert!(e.message.contains(skip), "{}", e.message);
        assert_eq!(hits.load(Ordering::SeqCst), 0, "к узлу не было запросов");
    }
}

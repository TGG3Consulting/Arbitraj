//! Файл: tests/bsc_dex_inflows.rs
//! Что делает: ожидаемые поступления в кошелёк DEX и обмен с тем же токеном на выходе не идут одновременно
//! (wallet/inflows.rs): обмен ждёт, зачисление и отказ вывода снимают ожидание, повтор статуса — один раз на хеш.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::wallet::inflows, arb::settings (настоящая SQLite во временной папке).

use std::time::Duration;

use alloy_primitives::{Address, address};
use serde_json::json;
use tokio::sync::broadcast;

use arb::settings::scope::Scope;
use arb::settings::store::Store;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use arb::storage::raw_log::RawLog;
use arb::venues::bsc_dex::ctx::Ctx;
use arb::venues::bsc_dex::wallet::inflows::Inflows;

const COIN: Address = address!("0x1000000000000000000000000000000000000001");
const USDT: Address = address!("0x2000000000000000000000000000000000000002");

/// Окружение адаптера на настоящей SQLite во временной папке.
async fn ctx(name: &str) -> Ctx {
    let d = std::env::temp_dir().join(format!("arb-dex-in-{name}-{}", std::process::id()));
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

/// Задаёт настройку площадки bsc_dex.
async fn set(c: &Ctx, name: &str, v: serde_json::Value) {
    c.settings
        .set(name, Scope::Venue("bsc_dex".into()), Some(v), "тест")
        .await
        .unwrap();
}

#[tokio::test]
async fn inflow_and_swap_do_not_overlap() {
    let c = ctx("inflow").await;
    set(&c, "transfer.max_time_s", json!(3600)).await;
    set(&c, "dex.inflow_wait_notify_s", json!(3600)).await;
    let f = Inflows::default();
    f.expect(USDT).await;
    // Ожидается поступление USDT — обмен с USDT на выходе ждёт.
    let waiting = tokio::time::timeout(Duration::from_millis(50), f.swap(&c, USDT, &|_| {})).await;
    assert!(waiting.is_err());
    // Другой токен не ждёт.
    drop(f.swap(&c, COIN, &|_| {}).await.unwrap());
    // Зачислено — обмен идёт; повторный статус того же перевода не снимает чужое ожидание.
    let tx = "0x07";
    f.done(USDT, tx);
    let guard = f.swap(&c, USDT, &|_| {}).await.unwrap();
    // Пока идёт обмен, адрес депозита USDT ждёт.
    let dep = tokio::time::timeout(Duration::from_millis(50), f.expect(USDT)).await;
    assert!(dep.is_err());
    drop(guard);
    f.expect(USDT).await;
    f.done(USDT, tx);
    let still = tokio::time::timeout(Duration::from_millis(50), f.swap(&c, USDT, &|_| {})).await;
    assert!(
        still.is_err(),
        "повторный статус того же хеша не снимает новое ожидание"
    );
}

#[tokio::test]
async fn abandoned_withdrawal_releases_swap() {
    let c = ctx("abandon").await;
    set(&c, "transfer.max_time_s", json!(3600)).await;
    set(&c, "dex.inflow_wait_notify_s", json!(3600)).await;
    let f = Inflows::default();
    f.expect(COIN).await;
    // Вывод на кошелёк не состоялся — ожидание снято, обмен идёт сразу.
    f.abandoned(COIN);
    let ok = tokio::time::timeout(Duration::from_millis(50), f.swap(&c, COIN, &|_| {})).await;
    assert!(ok.is_ok());
}

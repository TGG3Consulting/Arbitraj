//! Файл: tests/accounts_chain_exec_432.rs
//! Что делает: решение 432 (шаг 4 двух сетей) — кошелёк один на сети, исполнитель и очередь nonce по площадке DEX сети
//! (settings/accounts/chain_exec.rs) на настоящей SQLite во временной папке: (а) кошелёк переноса (queue_key «bsc_dex»,
//! как в main.rs) — домашняя площадка bsc_dex: запись глазами bsc_dex байт в байт прежняя, исполнитель — прежние поля;
//! (б) исполнитель на другой площадке — своя строка, у домашней не меняется; ключ очереди «<площадка>:<id>»; (в) версия
//! после сверки — только при том же адресе; (г) переживает перезапуск; (д) снять — строки нет.
//! Адрес исполнителя — записанный адрес развёрнутого ArbExecutor из решения 132 (0x9ef5…f794, сервер 03.11.2026).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::settings::{store, accounts}, arb::storage. Моков нет.

use tokio::sync::broadcast;

use arb::settings::accounts::ROW_PROBE;
use arb::settings::accounts::migrate::Legacy;
use arb::settings::store::Store;
use arb::storage::db::Db;
use arb::storage::journal::Journal;

/// Записанный адрес исполнителя из решения 132.
const EXECUTOR: &str = "0x9ef558a5d437718dde44e00990e0efb9976cf794";

/// Хранилище на базе в папке (та же папка — «перезапуск»).
async fn store(d: &std::path::Path) -> Store {
    let db = Db::open(&d.join("t.sqlite")).unwrap();
    let (changes, _) = broadcast::channel(64);
    let (jtx, _) = broadcast::channel(64);
    Store::load(db.clone(), changes, Journal::new(db, jtx))
        .await
        .unwrap()
}

#[tokio::test]
async fn executor_and_queue_by_dex_venue() {
    let d = std::env::temp_dir().join(format!("arb-chain-exec-432-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let s = store(&d).await;
    let a = s.accounts();
    let legacy = Legacy {
        name: "BingX",
        secret: "bsc_dex.wallet_key",
        queue_key: "bsc_dex",
        executor: Some(EXECUTOR.to_string()),
        rows: &["bingx", ROW_PROBE],
    };
    let w = a.migrate(true, legacy, "t").await.unwrap().unwrap();
    let id = w.id.clone();
    // (а) домашняя площадка — запись прежняя.
    assert_eq!(w.home(), "bsc_dex");
    assert_eq!(w.at("bsc_dex"), w);
    assert_eq!(w.executor.as_deref(), Some(EXECUTOR));
    assert!(w.chains.is_empty());
    let js = serde_json::to_value(&w).unwrap();
    assert!(js.get("chains").is_none(), "JSON кошелька прежний: {js}");
    // (б) другая площадка — исполнителя нет, очередь своя.
    let arb = a.wallet_at("arb_dex", &id).unwrap();
    assert_eq!(arb.executor, None);
    assert_eq!(arb.queue_key, format!("arb_dex:{id}"));
    let x = a
        .set_executor_at("arb_dex", &id, Some(EXECUTOR.into()), "тест")
        .await
        .unwrap();
    assert_eq!(x.executor.as_deref(), Some(EXECUTOR));
    assert_eq!(x.executor_version, None, "не сверен");
    let home = a.wallet_at("bsc_dex", &id).unwrap();
    assert_eq!(home.executor.as_deref(), Some(EXECUTOR));
    assert_eq!(home.queue_key, "bsc_dex", "ключ очереди BSC прежний");
    // Снимок без замка (решение 401) видит то же.
    let seen = a.with_wallet_at("arb_dex", Some(&id), |w| w.and_then(|w| w.executor.clone()));
    assert_eq!(seen.as_deref(), Some(EXECUTOR));
    // (в) версия — только при том же адресе.
    let other = "0x0000000000000000000000000000000000000001";
    let y = a
        .set_executor_version_at("arb_dex", &id, other, Some("v3".into()), None, "тест")
        .await
        .unwrap();
    assert_eq!(y.executor_version, None, "адрес не тот — версия не пишется");
    let y = a
        .set_executor_version_at("arb_dex", &id, EXECUTOR, Some("v3".into()), None, "тест")
        .await
        .unwrap();
    assert_eq!(y.executor_version.as_deref(), Some("v3"));
    assert_eq!(a.wallet_at("bsc_dex", &id).unwrap().executor_version, None);
    // (г) перезапуск.
    drop(s);
    let s = store(&d).await;
    let a = s.accounts();
    let y = a.wallet_at("arb_dex", &id).unwrap();
    assert_eq!(y.executor.as_deref(), Some(EXECUTOR));
    assert_eq!(y.executor_version.as_deref(), Some("v3"));
    assert_eq!(a.wallet(&id).unwrap().executor.as_deref(), Some(EXECUTOR));
    // (д) снять.
    a.set_executor_at("arb_dex", &id, None, "тест")
        .await
        .unwrap();
    assert!(a.wallet(&id).unwrap().chains.is_empty());
    assert_eq!(a.wallet_at("arb_dex", &id).unwrap().executor, None);
}

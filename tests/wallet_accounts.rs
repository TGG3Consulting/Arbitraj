//! Файл: tests/wallet_accounts.rs
//! Что делает: кошельки DEX и маппинг (решение 132, settings/accounts/) на настоящей SQLite во временной папке:
//! перенос старого кошелька (один раз; без ключа — не делается; уже есть кошельки — не делается), переживает
//! перезапуск; запрет удаления кошелька в маппинге и в незавершённой сделке; уникальность имени; «деньги смешиваются»;
//! история «было/стало». Исполнитель переноса — записанный адрес развёрнутого ArbExecutor из решения 132
//! (0x9ef558a5d437718dde44e00990e0efb9976cf794, exec.executor_address на сервере 03.11.2026).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::settings::{store, accounts}, arb::storage.

use tokio::sync::broadcast;

use arb::settings::accounts::migrate::Legacy;
use arb::settings::accounts::{ROW_DEX_DEX, ROW_PROBE};
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
    Store::load(
        db,
        changes,
        Journal::new(Db::open(&d.join("t.sqlite")).unwrap(), jtx),
    )
    .await
    .unwrap()
}

/// Чистая временная папка теста.
fn dir(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("arb-wallets-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Что переносится в тесте — как в main.rs.
fn legacy(rows: &'static [&'static str]) -> Legacy<'static> {
    Legacy {
        name: "BingX",
        secret: "bsc_dex.wallet_key",
        queue_key: "bsc_dex",
        executor: Some(EXECUTOR.to_string()),
        rows,
    }
}

#[tokio::test]
async fn migrate_once_with_executor_and_mapping() {
    let d = dir("migrate");
    let s = store(&d).await;
    let a = s.accounts();
    // Ключа в секретах нет — переноса нет и отметки нет (повторится при следующей разблокировке).
    assert!(
        a.migrate(false, legacy(&["bingx", ROW_PROBE]), "t")
            .await
            .unwrap()
            .is_none()
    );
    assert!(a.wallets().is_empty());
    let w = a
        .migrate(true, legacy(&["bingx", ROW_PROBE]), "t")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(w.name, "BingX");
    assert_eq!(
        w.secret, "bsc_dex.wallet_key",
        "ключ не переписывается — тот же секрет"
    );
    assert_eq!(w.executor.as_deref(), Some(EXECUTOR));
    assert_eq!(
        w.queue_key, "bsc_dex",
        "очередь — прежняя строка wallet_queue"
    );
    assert_eq!(a.row("bingx"), Some(w.id.clone()));
    assert_eq!(a.row(ROW_PROBE), Some(w.id.clone()));
    assert_eq!(a.row("mexc"), None, "MEXC — без маппинга");
    assert_eq!(a.row(ROW_DEX_DEX), None, "DEX ↔ DEX — без маппинга");
    assert_eq!(a.legacy(), Some(w.id.clone()));
    // Второй раз — ничего (один раз).
    assert!(
        a.migrate(true, legacy(&["bingx"]), "t")
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(a.wallets().len(), 1);
    // «Перезапуск»: всё прочитано из базы, перенос не повторяется.
    let s2 = store(&d).await;
    let a2 = s2.accounts();
    assert_eq!(a2.wallets(), vec![w.clone()]);
    assert_eq!(a2.legacy(), Some(w.id.clone()));
    assert!(
        a2.migrate(true, legacy(&["bingx"]), "t")
            .await
            .unwrap()
            .is_none()
    );
    // Строка «пробы» в «деньги смешиваются» не входит.
    assert!(a2.shared().is_empty());
    let h = a2.history(50).await.unwrap();
    assert!(h.iter().any(|x| x["what"] == "wallet.add"));
    assert!(
        h.iter()
            .any(|x| x["what"] == "wallet.executor" && x["new"] == EXECUTOR)
    );
    assert!(
        h.iter()
            .any(|x| x["what"] == "map" && x["target"] == "bingx")
    );
}

#[tokio::test]
async fn no_migration_when_wallets_exist() {
    let d = dir("exist");
    let s = store(&d).await;
    let a = s.accounts();
    let id = a.reserve_id().await.unwrap();
    let secret = arb::settings::accounts::edit_map::secret_name(&id);
    a.add(&id, "MEXC", &secret, "wallet:x", "t").await.unwrap();
    assert!(
        a.migrate(true, legacy(&["bingx"]), "t")
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(a.wallets().len(), 1);
    assert_eq!(a.row("bingx"), None);
}

#[tokio::test]
async fn remove_forbidden_while_mapped_or_busy() {
    let d = dir("remove");
    let s = store(&d).await;
    let a = s.accounts();
    let id1 = a.reserve_id().await.unwrap();
    let id2 = a.reserve_id().await.unwrap();
    assert_ne!(id1, id2, "id не повторяются");
    a.add(&id1, "MEXC", "wallet.a.key", "wallet:a", "t")
        .await
        .unwrap();
    assert!(
        a.add(&id2, " mexc ", "wallet.b.key", "wallet:b", "t")
            .await
            .is_err(),
        "имя занято"
    );
    a.add(&id2, "Циклы", "wallet.b.key", "wallet:b", "t")
        .await
        .unwrap();
    a.map_set("mexc", Some(&id1), "t").await.unwrap();
    a.map_set(ROW_DEX_DEX, Some(&id1), "t").await.unwrap();
    // Один кошелёк на двух денежных строках — предупреждение «деньги смешиваются».
    let shared = a.shared();
    assert_eq!(shared.len(), 1);
    assert_eq!(
        shared[0].1,
        vec![ROW_DEX_DEX.to_string(), "mexc".to_string()]
    );
    // В маппинге — удалить нельзя.
    let e = a.remove(&id1, &[], "t").await.unwrap_err().to_string();
    assert!(e.contains("маппинге"), "{e}");
    // В незавершённой сделке — удалить нельзя.
    let busy = vec!["20261103185211293CGPT".to_string()];
    let e = a.remove(&id2, &busy, "t").await.unwrap_err().to_string();
    assert!(e.contains("незавершённые сделки"), "{e}");
    // Привязка к несуществующему кошельку — отказ.
    assert!(a.map_set("bingx", Some("w999"), "t").await.is_err());
    // Отвязали и нет сделок — удаляется; запись истории «было».
    a.map_set("mexc", None, "t").await.unwrap();
    a.map_set(ROW_DEX_DEX, None, "t").await.unwrap();
    a.remove(&id1, &[], "t").await.unwrap();
    assert!(a.wallet(&id1).is_none());
    let h = a.history(50).await.unwrap();
    assert!(
        h.iter()
            .any(|x| x["what"] == "wallet.remove" && x["old"]["name"] == "MEXC")
    );
}

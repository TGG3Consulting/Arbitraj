//! Файл: tests/settings_dex_scopes.rs
//! Что делает: проверяет новые виды значений и области настроек на настоящей базе SQLite:
//! выбор из вариантов, адрес BSC, верхнюю границу целого, области «DEX» и «канал» (без подстановки),
//! переживание перезапуска (запись области в базе и разбор обратно).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::settings, arb::storage. Моков нет — настоящая база; данных площадок здесь нет.

use arb::gateway::errors::ErrorCode;
use arb::settings::store::Store;
use arb::settings::value::Scope;
use arb::storage::db::Db;
use serde_json::json;
use tokio::sync::broadcast;

/// Открывает (или создаёт) базу в папке теста. Возвращает хранилище.
async fn open(dir: &std::path::Path) -> Store {
    std::fs::create_dir_all(dir).unwrap();
    let db = Db::open(&dir.join("t.sqlite")).unwrap();
    let (tx, _) = broadcast::channel(16);
    let (jtx, _) = broadcast::channel(16);
    let journal = arb::storage::journal::Journal::new(db.clone(), jtx);
    Store::load(db, tx, journal).await.unwrap()
}

/// Папка теста, пустая.
fn dir(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("arb-test-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

#[tokio::test]
async fn choice_address_and_max_are_checked() {
    let s = open(&dir("kinds")).await;
    let v = Scope::Venue("bsc_dex".into());
    assert!(
        s.set("dex.swap_mode", v.clone(), Some(json!("smart")), "т")
            .await
            .is_ok()
    );
    assert!(
        s.set("dex.swap_mode", v.clone(), Some(json!("fast")), "т")
            .await
            .is_err()
    );
    assert!(
        s.set("smart.permit2", v.clone(), Some(json!("0x12")), "т")
            .await
            .is_err()
    );
    let wait_max = json!(3 * 60 * 60 * 1000_u64);
    assert!(
        s.set("stuck.wait_ms", Scope::Global, Some(wait_max), "т")
            .await
            .is_ok()
    );
    let over = json!(3 * 60 * 60 * 1000_u64 + 1);
    assert!(
        s.set("stuck.wait_ms", Scope::Global, Some(over), "т")
            .await
            .is_err()
    );
}

#[tokio::test]
async fn dex_and_channel_scopes_are_exact_and_persist() {
    let d = dir("scopes");
    {
        let s = open(&d).await;
        let ch = Scope::Channel("club48".into());
        s.set("channel.min_gas_price_gwei", ch, Some(json!("0.05")), "т")
            .await
            .unwrap();
        // Канал не задаётся для всей системы.
        assert!(
            s.set("channel.enabled", Scope::Global, Some(json!(true)), "т")
                .await
                .is_err()
        );
        let fam = Scope::Dex("pancakeswap_v3".into());
        s.set("dex.pool_enabled", fam, Some(json!(true)), "т")
            .await
            .unwrap();
    }
    let s = open(&d).await;
    let min = s
        .decimal_in(
            "channel.min_gas_price_gwei",
            &Scope::Channel("club48".into()),
        )
        .unwrap();
    assert_eq!(min.to_string(), "0.05");
    let other = s
        .decimal_in(
            "channel.min_gas_price_gwei",
            &Scope::Channel("blockrazor".into()),
        )
        .unwrap_err();
    assert_eq!(other.code, ErrorCode::NotConfigured);
    assert!(
        s.flag_in("dex.pool_enabled", &Scope::Dex("pancakeswap_v3".into()))
            .unwrap()
    );
}

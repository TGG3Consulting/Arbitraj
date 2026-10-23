//! Файл: tests/settings_store.rs
//! Что делает: проверяет хранилище настроек на настоящей базе SQLite во временной папке:
//! проверку значений, подстановку области, историю, рассылку изменений, очистку.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::settings, arb::storage. Моков нет — настоящая база.

use arb::settings::read::At;
use arb::settings::store::Store;
use arb::settings::value::Scope;
use arb::storage::db::Db;
use serde_json::json;
use tokio::sync::broadcast;

/// Новая база во временной папке. Возвращает хранилище и приёмник изменений.
async fn fresh(name: &str) -> (Store, broadcast::Receiver<arb::settings::live::Change>) {
    let dir = std::env::temp_dir().join(format!("arb-test-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let db = Db::open(&dir.join("t.sqlite")).unwrap();
    let (tx, rx) = broadcast::channel(16);
    (Store::load(db, tx).await.unwrap(), rx)
}

#[tokio::test]
async fn not_set_means_not_configured() {
    let (s, _) = fresh("notset").await;
    let err = s
        .decimal("money.min_profit_usd", At::default())
        .unwrap_err();
    assert_eq!(err.code, arb::gateway::errors::ErrorCode::NotConfigured);
}

#[tokio::test]
async fn set_resolve_history_and_live() {
    let (s, mut rx) = fresh("live").await;
    s.set(
        "feed.stale_after_ms",
        Scope::Venue("bingx".into()),
        Some(json!(1500)),
        "тест",
    )
    .await
    .unwrap();
    let ch = rx.recv().await.unwrap();
    assert_eq!(ch.name, "feed.stale_after_ms");
    assert_eq!(
        s.integer(
            "feed.stale_after_ms",
            At {
                venue: Some("bingx"),
                coin: None
            }
        )
        .unwrap(),
        1500
    );
    // Для другой площадки значения нет: подстановки из другой площадки не бывает.
    assert!(
        s.integer(
            "feed.stale_after_ms",
            At {
                venue: Some("other"),
                coin: None
            }
        )
        .is_err()
    );
    let h = s
        .history(Some("feed.stale_after_ms".into()), 10)
        .await
        .unwrap();
    assert_eq!(h.len(), 1);
}

#[tokio::test]
async fn coin_beats_venue_beats_global() {
    let (s, _) = fresh("scope").await;
    s.set(
        "money.max_per_trade_usd",
        Scope::Global,
        Some(json!("100")),
        "тест",
    )
    .await
    .unwrap();
    s.set(
        "money.max_per_trade_usd",
        Scope::Venue("bingx".into()),
        Some(json!("50.5")),
        "тест",
    )
    .await
    .unwrap();
    let g = s.decimal("money.max_per_trade_usd", At::default()).unwrap();
    let v = s
        .decimal(
            "money.max_per_trade_usd",
            At {
                venue: Some("bingx"),
                coin: None,
            },
        )
        .unwrap();
    assert_eq!(g.to_string(), "100");
    assert_eq!(v.to_string(), "50.5");
}

#[tokio::test]
async fn rejects_bad_values_and_scopes() {
    let (s, _) = fresh("bad").await;
    assert!(
        s.set(
            "profit.liquidity_share_h",
            Scope::Venue("x".into()),
            Some(json!("1.5")),
            "т"
        )
        .await
        .is_err()
    );
    assert!(
        s.set(
            "profit.liquidity_share_h",
            Scope::Global,
            Some(json!("0.5")),
            "т"
        )
        .await
        .is_err()
    );
    assert!(
        s.set(
            "bsc.node_ws_url",
            Scope::Global,
            Some(json!("ftp://x")),
            "т"
        )
        .await
        .is_err()
    );
    assert!(
        s.set("no.such.setting", Scope::Global, Some(json!(1)), "т")
            .await
            .is_err()
    );
}

#[tokio::test]
async fn clear_removes_value() {
    let (s, _) = fresh("clear").await;
    s.set(
        "ledger.reconcile_interval_s",
        Scope::Global,
        Some(json!(60)),
        "т",
    )
    .await
    .unwrap();
    s.set("ledger.reconcile_interval_s", Scope::Global, None, "т")
        .await
        .unwrap();
    assert!(
        s.integer("ledger.reconcile_interval_s", At::default())
            .is_err()
    );
}

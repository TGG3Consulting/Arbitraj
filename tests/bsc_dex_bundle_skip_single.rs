//! Файл: tests/bsc_dex_bundle_skip_single.rs
//! Что делает: решение 299 — отбор каналов для отправки бандла при send.bundle_skip_single. Набор каналов — как у
//! живого бота 15.11.2026: club48, jetbuilder, nodereal в режиме бандла + blockrazor (BlockRazor RPC, только одиночная).
//! Проверяется: не задано / нет — прежний отказ котировки (send::planned_bundle_ok) и каналы не фильтруются; да — бандл
//! исполним, одиночный исключён, граница газа — по бандловым; да и ни одного бандлового — отказ с понятной причиной.
//! Без сети: читается только готовность каналов (send::ready::prepare) — eth_sendBundle не отправляется (не покрыто).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::{send, ctx}, arb::settings (настоящая SQLite во временной папке).

use std::path::PathBuf;

use alloy_primitives::U256;
use serde_json::json;
use tokio::sync::broadcast;

use arb::core::types::SendMethod;
use arb::gateway::errors::ErrorCode;
use arb::settings::scope::Scope;
use arb::settings::store::Store;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use arb::storage::raw_log::RawLog;
use arb::venues::bsc_dex::ctx::Ctx;
use arb::venues::bsc_dex::send::{self, bundle_pick, stats::Stats};

/// Окружение адаптера на настоящей SQLite во временной папке (как tests/bsc_dex_send.rs).
async fn ctx(name: &str) -> Ctx {
    let d: PathBuf = std::env::temp_dir().join(format!("arb-skip-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let db = Db::open(&d.join("t.sqlite")).unwrap();
    let (changes, _) = broadcast::channel(64);
    let (jtx, _) = broadcast::channel(64);
    let journal = Journal::new(db.clone(), jtx);
    let settings = Store::load(db, changes, journal.clone()).await.unwrap();
    {
        let c = Ctx {
            raw: RawLog::start(d.join("raw"), settings.clone()),
            pace: Default::default(),
            failover: Default::default(),
            chains: Default::default(),
            account: None,
            index: Default::default(),
            settings,
            vault: arb::secrets::Vault::locked(d.join("secrets.enc")),
            http: reqwest::Client::new(),
            journal,
        };
        // Решение 416: тикер газа площадки DEX — настройка (dex.gas_ticker).
        c.settings
            .set(
                "dex.gas_ticker",
                arb::settings::scope::Scope::Venue("bsc_dex".into()),
                Some(serde_json::json!("BNB")),
                "тест",
            )
            .await
            .unwrap();
        c
    }
}

/// Готовый канал `id` способом `method` с минимальной ценой газа `gwei` (без ключа и чаевых — каналы без ключа).
async fn channel(c: &Ctx, id: &str, method: &str, gwei: &str) {
    let s = Scope::Channel(id.into());
    for (n, v) in [
        ("channel.enabled", json!(true)),
        ("channel.url", json!("https://example.invalid")),
        ("channel.method", json!(method)),
        ("channel.min_gas_price_gwei", json!(gwei)),
        ("channel.tip_enabled", json!(false)),
    ] {
        c.settings.set(n, s.clone(), Some(v), "тест").await.unwrap();
    }
}

/// Задаёт send.bundle_skip_single у площадки bsc_dex.
async fn skip(c: &Ctx, on: bool) {
    c.settings
        .set(
            "send.bundle_skip_single",
            Scope::Venue("bsc_dex".into()),
            Some(json!(on)),
            "тест",
        )
        .await
        .unwrap();
}

/// Каналы живого бота 15.11.2026: три бандловых и BlockRazor RPC одиночной (минимум газа выше — для проверки границы).
async fn live_channels(c: &Ctx) {
    channel(c, "club48", "bundle", "0.05").await;
    channel(c, "jetbuilder", "bundle", "0.05").await;
    channel(c, "nodereal", "bundle", "0.05").await;
    channel(c, "blockrazor", "single", "1").await;
}

/// Не задано и «нет» — прежнее поведение: отказ котировки с прежней причиной, каналы для бандла не фильтруются.
#[tokio::test]
async fn unset_or_off_keeps_old_refusal() {
    let c = ctx("unset").await;
    live_channels(&c).await;
    for step in [None, Some(false)] {
        if let Some(on) = step {
            skip(&c, on).await;
        }
        let why = send::planned_bundle_ok(&c).unwrap_err();
        assert!(
            why.contains("не в режиме бандла") && why.contains("BlockRazor RPC"),
            "{why}"
        );
        let picked = bundle_pick::pick(&c, send::select(&c, &Stats::default()).unwrap()).unwrap();
        assert_eq!(picked.ready.len(), 4, "каналы не фильтруются");
        assert!(picked.skipped.is_empty());
    }
}

/// Да — бандл проходит: 3 бандловых, одиночный исключён, граница газа по бандловым (0,05 gwei, а не 1).
#[tokio::test]
async fn on_sends_to_bundle_channels_only() {
    let c = ctx("on").await;
    live_channels(&c).await;
    skip(&c, true).await;
    assert_eq!(send::planned_bundle_ok(&c), Ok(()));
    let all = send::select(&c, &Stats::default()).unwrap();
    assert_eq!(send::gas_floor(&all), U256::from(1_000_000_000u64));
    let picked = bundle_pick::pick(&c, all).unwrap();
    let mut ids: Vec<&str> = picked.ready.iter().map(|r| r.ch.id).collect();
    ids.sort();
    assert_eq!(ids, ["club48", "jetbuilder", "nodereal"]);
    assert!(picked.ready.iter().all(|r| r.method == SendMethod::Bundle));
    assert_eq!(picked.skipped, ["BlockRazor RPC"]);
    assert_eq!(send::gas_floor(&picked.ready), U256::from(50_000_000u64));
    // Одиночная отправка не меняется: send::select по-прежнему отдаёт все готовые каналы.
    assert_eq!(send::select(&c, &Stats::default()).unwrap().len(), 4);
}

/// Да, но ни одного бандлового — отказ котировки и отправки с понятной причиной.
#[tokio::test]
async fn on_without_bundle_channel_refuses() {
    let c = ctx("none").await;
    channel(&c, "blockrazor", "single", "1").await;
    skip(&c, true).await;
    let why = send::planned_bundle_ok(&c).unwrap_err();
    assert!(
        why.contains("send.bundle_skip_single")
            && why.contains("нет ни одного в режиме бандла")
            && why.contains("BlockRazor RPC"),
        "{why}"
    );
    let e = match bundle_pick::pick(&c, send::select(&c, &Stats::default()).unwrap()) {
        Ok(_) => panic!("бандловых нет — должен быть отказ"),
        Err(e) => e,
    };
    assert_eq!(e.code, ErrorCode::NotConfigured);
    assert!(
        e.message.contains("нет ни одного в режиме бандла"),
        "{}",
        e.message
    );
}

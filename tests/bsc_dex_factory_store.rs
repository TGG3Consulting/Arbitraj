//! Файл: tests/bsc_dex_factory_store.rs
//! Что делает: ревью арбитра 28.11.2026, Б4 — пулы поиска по фабрикам сохраняются в SQLite (directory/store_factory.rs) и
//! восстанавливаются в справочник фабрик (Snap.factory) при старте подписки, до стартовой проверки на узле. Живой бот
//! 28.11.2026 (base_dex): справочник индексатора пуст (код поставщика 10006) — без сохранения каждый перезапуск начинался с
//! нуля пулов. Строки — пулы CAKE из записи GeckoTerminal 2026-10-24 (tests/data/indexers_2026-10-24), в том виде, в каком
//! справочник хранит найденное (IndexedPool).
//!
//! Кто вызывает: cargo test.
//! От чего зависит: tests/native_common, arb::venues::bsc_dex::directory::{gt_parse, store_factory}.

mod native_common;

use std::collections::HashMap;

use alloy_primitives::address;
use serde_json::{Value, json};

use arb::settings::scope::Scope;
use arb::venues::bsc_dex::directory::{gt_parse, store_factory};

#[tokio::test]
async fn factory_pools_survive_restart() {
    let ctx = native_common::ctx("factory-store").await;
    let text =
        std::fs::read_to_string("tests/data/indexers_2026-10-24/arb_indexers_2026-10-24.json")
            .unwrap();
    let rec: Value = serde_json::from_str(&text).unwrap();
    let rows = gt_parse::pools(rec["gt_pools_CAKE_p1"]["body"].as_str().unwrap()).unwrap();
    assert!(!rows.is_empty());
    let cake = address!("0x0E09FaBB73Bd3Ade0a17ECC321fD13a19e81cE82");
    let coins = HashMap::from([(cake, "CAKE".to_string())]);
    let v = Scope::Venue(ctx.place.id.into());
    ctx.settings
        .set("dex.evm_chain_id", v.clone(), Some(json!(56)), "тест")
        .await
        .unwrap();
    ctx.settings
        .set("index.factory_scan", v.clone(), Some(json!(true)), "тест")
        .await
        .unwrap();
    store_factory::save(&ctx, vec![(cake, rows.clone())])
        .await
        .unwrap();
    // «Перезапуск»: справочник фабрик в памяти пуст.
    ctx.index.with(|s| s.factory.clear());
    let n = store_factory::restore(&ctx, &coins).await.unwrap();
    assert_eq!(n, rows.len());
    assert_eq!(
        ctx.index.with(|s| s.factory.get(&cake).cloned()),
        Some(rows.clone())
    );
    // Повторное восстановление не дублирует строки.
    assert_eq!(store_factory::restore(&ctx, &coins).await.unwrap(), 0);
    // Монета не из подписки — не восстанавливается.
    ctx.index.with(|s| s.factory.clear());
    let other = HashMap::from([(
        address!("0x2170Ed0880ac9A755fd29B2688956BD959F933F8"),
        "ETH".to_string(),
    )]);
    assert_eq!(store_factory::restore(&ctx, &other).await.unwrap(), 0);
    // Поиск по фабрикам выключен — сохранённое не восстанавливается.
    ctx.settings
        .set("index.factory_scan", v, Some(json!(false)), "тест")
        .await
        .unwrap();
    assert_eq!(store_factory::restore(&ctx, &coins).await.unwrap(), 0);
}

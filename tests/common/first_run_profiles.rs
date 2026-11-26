//! Рекомендованные значения в справочнике первого запуска проверяются отдельно от настроек адаптера.
use alloy_primitives::B256;
use serde_json::Value;

use std::sync::OnceLock;

/// Строка рекомендаций для одной фабрики на указанной сети.
pub fn named(chain: u64, id: &str) -> Option<&'static Value> {
    static ROWS: OnceLock<Vec<Value>> = OnceLock::new();
    ROWS.get_or_init(|| {
        serde_json::from_str(include_str!("../../config/first_run_dex.json")).unwrap()
    })
    .iter()
    .find(|r| r["chain_id"] == chain && r["id"] == id)
}

#[allow(dead_code)] // отдельные интеграционные тесты используют только named
pub fn recommended(chain: u64, id: &str) -> Vec<B256> {
    named(chain, id)
        .and_then(|r| r["settings"]["dex.module_runtime_hashes"].as_array())
        .into_iter()
        .flatten()
        .map(|v| v.as_str().unwrap().parse().unwrap())
        .collect()
}

//! Файл: venues/bsc_dex/pools/ekubo_shallow.rs
//! Что делает: подписка на события пулов Ekubo (решение 447). У ядра PoolId не в topics, а обмен — анонимный log0 без
//! topics: фильтр «PoolId в topics[1]» (V4) и «topic0 из списка» его не пропустят. Поэтому:
//! - подписка «все события» пачки Ekubo — по адресу ядра без topics (все логи ядра; пул выбирает ekubo_route.rs);
//! - подписка «только ликвидность» — по адресу ядра и topic0 PositionUpdated;
//! - лог «всех событий» до мелкого пула того же ядра будит его только событием ликвидности (как мелкий V2 — Mint).
//!
//! Кто вызывает: pools/group.rs (filter), pools/session.rs (wakes).
//! От чего зависит: families.rs, events.rs, ekubo_route.rs, discover.rs (Found).
//! Настройки: нет.
//!
//! Фильтр logs: address — адрес; topics не задан — любые логи адреса (execution-apis
//! 16caee274b0d4fb7a0316f294029adac88a75a09 src/eth/subscribe.yaml L1-L60, тот же фильтр, что у eth_getLogs).

use alloy::rpc::types::Log;
use serde_json::{Value, json};

use crate::core::types::PoolRef;

use super::discover::Found;
use super::events;
use super::families::{Family, Proto};

/// Фильтр подписки пачки Ekubo. Принимает: первый пул пачки (все пулы пачки одного вида), только ли ликвидность.
/// Возвращает: None — пачка не Ekubo (общий путь group.rs). Ошибок нет.
pub fn filter(first: &Found, liquidity: bool) -> Option<Value> {
    if first.family.proto != Proto::Ekubo {
        return None;
    }
    let PoolRef::Singleton { manager, .. } = first.reference else {
        return None;
    };
    Some(if liquidity {
        json!({ "address": manager, "topics": [events::liquidity_topics(first.family)] })
    } else {
        json!({ "address": manager })
    })
}

/// Будит ли лог мелкий пул: у Ekubo — только событие ликвидности (подписка «все события» соседей по ядру приносит
/// и обмены); у других видов — как раньше (любой лог их подписки ликвидности). Ошибок нет.
pub fn wakes(f: &Family, log: &Log) -> bool {
    if f.proto != Proto::Ekubo {
        return true;
    }
    super::ekubo_route::topic(log).is_some_and(|t| events::liquidity_topics(f).contains(&t))
}

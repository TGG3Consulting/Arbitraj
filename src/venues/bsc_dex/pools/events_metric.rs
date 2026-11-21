//! Файл: venues/bsc_dex/pools/events_metric.rs
//! Что делает: события пула Metric OMM (решение 285) — topic0 для подписки и применение: любое известное событие пула
//! (topic0 из записанных логов BSC, abi_metric.rs topics) → перечитать пул целиком (цены провайдера и лесенка на
//! блоке события). Цена провайдера меняется без событий пула — вид ещё перечитывается каждым блоком (per_block.rs).
//! Кто вызывает: pools/events.rs.
//! От чего зависит: abi_metric.rs, events.rs, registry.rs.
//! Настройки: нет.

use alloy::rpc::types::Log;
use alloy_primitives::B256;

use super::abi_metric;
use super::events::Applied;
use super::registry::Entry;

/// topic0 всех событий пула. Ошибок нет.
pub fn topics() -> Vec<B256> {
    abi_metric::topics()
}

/// topic0 событий изменения ликвидности. Ошибок нет.
pub fn liquidity_topics() -> Vec<B256> {
    abi_metric::liquidity_topics()
}

/// Применяет событие: известное событие пула → Refresh, иначе Ignored. Ошибок нет.
pub fn apply(e: &mut Entry, log: &Log) -> Applied {
    match log.topic0() {
        Some(t) if topics().contains(t) => Applied::Refresh(format!(
            "{}: событие пула Metric — перечитать состояние",
            e.family.title
        )),
        _ => Applied::Ignored,
    }
}

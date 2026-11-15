//! Файл: venues/bsc_dex/pools/events_izi.rs
//! Что делает: события пулов iZiSwap / Biswap V3 (решение 286) — topic0 для подписки и применение: любое известное
//! событие пула (abi_izi.rs topics) → перечитать пул целиком (state и лесенка ответов пула на блоке события). Вид ещё
//! перечитывается каждым блоком (per_block.rs): у Biswap V3 комиссия пула голосуемая, а событие ChangeLiquidityState
//! (тип в интерфейсе неоднозначен) не слушается.
//! Кто вызывает: pools/events.rs.
//! От чего зависит: abi_izi.rs, events.rs, registry.rs.
//! Настройки: нет.

use alloy::rpc::types::Log;
use alloy_primitives::B256;

use super::abi_izi;
use super::events::Applied;
use super::families::Proto;
use super::registry::Entry;

/// topic0 всех событий пулов вида. Принимает: протокол. Ошибок нет.
pub fn topics(p: Proto) -> Vec<B256> {
    abi_izi::topics(p)
}

/// topic0 изменения ликвидности / комиссии (мелкие пулы). Ошибок нет.
pub fn liquidity_topics(p: Proto) -> Vec<B256> {
    abi_izi::liquidity_topics(p)
}

/// Применяет событие: известное событие пула → Refresh, иначе Ignored. Ошибок нет.
pub fn apply(e: &mut Entry, log: &Log) -> Applied {
    match log.topic0() {
        Some(t) if topics(e.family.proto).contains(t) => Applied::Refresh(format!(
            "{}: событие пула iZiSwap — перечитать состояние",
            e.family.title
        )),
        _ => Applied::Ignored,
    }
}

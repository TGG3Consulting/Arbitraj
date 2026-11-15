//! Файл: venues/bsc_dex/pools/events_lb.rs
//! Что делает: события пула Liquidity Book V2.1 (решение 290) — topic0 для подписки и применение: любое известное
//! событие пула (Swap, DepositedToBins, WithdrawnFromBins, CompositionFees, CollectedProtocolFees, FlashLoan,
//! StaticFeeParametersSet, ForcedDecay) → перечитать пул целиком (параметры комиссии, активный бин и полоса бинов):
//! события несут резервы по бинам частями (упакованные bytes32), перечитывание точнее. Время блока вид ещё
//! перечитывает каждым блоком (per_block_lb.rs).
//! Кто вызывает: pools/events.rs.
//! От чего зависит: abi_lb.rs (topic0 и ссылки на исходник), events.rs, registry.rs.
//! Настройки: нет.

use alloy::rpc::types::Log;
use alloy_primitives::B256;

use super::abi_lb;
use super::events::Applied;
use super::registry::Entry;

/// topic0 всех событий пула (abi_lb.rs; ILBPair.sol L37-L78). Ошибок нет.
pub fn topics() -> Vec<B256> {
    abi_lb::topics()
}

/// topic0 событий изменения ликвидности (DepositedToBins, WithdrawnFromBins). Ошибок нет.
pub fn liquidity_topics() -> Vec<B256> {
    abi_lb::liquidity_topics()
}

/// Применяет событие: известное событие пула → Refresh, иначе Ignored. Ошибок нет.
pub fn apply(e: &mut Entry, log: &Log) -> Applied {
    match log.topic0() {
        Some(t) if topics().contains(t) => Applied::Refresh(format!(
            "{}: событие пула Liquidity Book — перечитать состояние",
            e.family.title
        )),
        _ => Applied::Ignored,
    }
}

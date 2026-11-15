//! Файл: venues/bsc_dex/pools/events_dmm.rs
//! Что делает: события пула KyberSwap Classic DMM (решение 287) — topic0 для подписки и применение: любое известное
//! событие пула (Sync, Swap, UpdateEMA, Mint, Burn) → перечитать пул целиком (getTradeInfo на блоке события). Sync несёт
//! резервы, но комиссия блока зависит от EMA объёма и номера блока — перечитывание точнее; вид ещё перечитывается
//! каждым блоком (pools/per_block.rs).
//! Кто вызывает: pools/events.rs.
//! От чего зависит: abi_dmm.rs (topic0 и ссылки на исходник), events.rs, registry.rs.
//! Настройки: нет.

use alloy::rpc::types::Log;
use alloy_primitives::B256;

use super::abi_dmm;
use super::events::Applied;
use super::registry::Entry;

/// topic0 всех событий пула (abi_dmm.rs; DMMPool.sol L50-L61, VolumeTrendRecorder.sol L24). Ошибок нет.
pub fn topics() -> Vec<B256> {
    abi_dmm::topics()
}

/// topic0 событий изменения ликвидности (Mint, Burn). Ошибок нет.
pub fn liquidity_topics() -> Vec<B256> {
    abi_dmm::liquidity_topics()
}

/// Применяет событие: известное событие пула → Refresh, иначе Ignored. Ошибок нет.
pub fn apply(e: &mut Entry, log: &Log) -> Applied {
    match log.topic0() {
        Some(t) if topics().contains(t) => Applied::Refresh(format!(
            "{}: событие пула KyberSwap DMM — перечитать состояние",
            e.family.title
        )),
        _ => Applied::Ignored,
    }
}

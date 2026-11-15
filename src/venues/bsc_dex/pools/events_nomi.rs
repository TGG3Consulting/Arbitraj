//! Файл: venues/bsc_dex/pools/events_nomi.rs
//! Что делает: события пары Nomiswap Stable (решение 284) — topic0 для подписки и применение: любое известное событие
//! пары (Sync, Swap, Mint, Burn, RampA, StopRampA) → перечитать пару целиком (резервы, swapFee, getA на блоке события).
//! Sync несёт резервы, но комиссия и A меняются без события (setSwapFee без emit, рампа A по времени блока) —
//! перечитывание точнее; вид ещё перечитывается каждым блоком (pools/per_block.rs).
//! Кто вызывает: pools/events.rs.
//! От чего зависит: abi_nomi.rs (topic0 и ссылки на verified-исходник), events.rs, registry.rs.
//! Настройки: нет.

use alloy::rpc::types::Log;
use alloy_primitives::B256;

use super::abi_nomi;
use super::events::Applied;
use super::registry::Entry;

/// topic0 всех событий пары (abi_nomi.rs; INomiswapPair.sol L8-L18, INomiswapStablePair.sol L9-L10). Ошибок нет.
pub fn topics() -> Vec<B256> {
    abi_nomi::topics()
}

/// topic0 событий изменения ликвидности (Mint, Burn). Ошибок нет.
pub fn liquidity_topics() -> Vec<B256> {
    abi_nomi::liquidity_topics()
}

/// Применяет событие: известное событие пары → Refresh, иначе Ignored. Ошибок нет.
pub fn apply(e: &mut Entry, log: &Log) -> Applied {
    match log.topic0() {
        Some(t) if topics().contains(t) => Applied::Refresh(format!(
            "{}: событие пары Nomiswap Stable — перечитать состояние",
            e.family.title
        )),
        _ => Applied::Ignored,
    }
}

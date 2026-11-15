//! Файл: venues/bsc_dex/pools/events_smardex.rs
//! Что делает: события пары SmarDex (решение 288) — topic0 для подписки и применение: любое известное событие пары
//! (Sync, Swap, FeesChanged, Mint, Burn) → перечитать пару целиком (резервы, фиктивные резервы, средняя цена, комиссии и
//! время блока). Sync несёт резервы и среднюю цену, но не время её записи и не комиссии — перечитывание точнее; вид ещё
//! перечитывается каждым блоком (pools/per_block.rs: средняя цена зависит от времени блока, резерв — от баланса пары).
//! Кто вызывает: pools/events.rs.
//! От чего зависит: abi_smardex.rs (topic0 и ссылки на исходник), events.rs, registry.rs.
//! Настройки: нет.

use alloy::rpc::types::Log;
use alloy_primitives::B256;

use super::abi_smardex;
use super::events::Applied;
use super::registry::Entry;

/// topic0 всех событий пары (abi_smardex.rs; ISmardexPair.sol L41-L87). Ошибок нет.
pub fn topics() -> Vec<B256> {
    abi_smardex::topics()
}

/// topic0 событий изменения ликвидности (Mint, Burn). Ошибок нет.
pub fn liquidity_topics() -> Vec<B256> {
    abi_smardex::liquidity_topics()
}

/// Применяет событие: известное событие пары → Refresh, иначе Ignored. Ошибок нет.
pub fn apply(e: &mut Entry, log: &Log) -> Applied {
    match log.topic0() {
        Some(t) if topics().contains(t) => Applied::Refresh(format!(
            "{}: событие пары SmarDex — перечитать состояние",
            e.family.title
        )),
        _ => Applied::Ignored,
    }
}

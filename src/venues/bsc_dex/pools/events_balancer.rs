//! Файл: venues/bsc_dex/pools/events_balancer.rs
//! Что делает: события пула Balancer V2 (решение 289) — topic0 для подписки и применение: любое известное событие самого
//! пула (SwapFeePercentageChanged, PausedStateChanged, AmpUpdateStarted, AmpUpdateStopped) → перечитать пул целиком.
//! Балансы пула меняет хранилище (Vault) своими событиями от СВОЕГО адреса — подписка по адресу пула их не видит, поэтому
//! вид ещё перечитывается каждым блоком (pools/per_block.rs). Событий ликвидности у самого пула нет (вход и выход
//! ликвидности — PoolBalanceChanged хранилища).
//! Кто вызывает: pools/events.rs.
//! От чего зависит: abi_balancer.rs (topic0 и ссылки на исходник), events.rs, registry.rs.
//! Настройки: нет.

use alloy::rpc::types::Log;
use alloy_primitives::B256;

use super::abi_balancer;
use super::events::Applied;
use super::registry::Entry;

/// topic0 событий пула (abi_balancer.rs; BasePool.sol L88, ITemporarilyPausable.sol L24, StablePool.sol L47-L48).
/// Ошибок нет.
pub fn topics() -> Vec<B256> {
    abi_balancer::topics()
}

/// topic0 событий изменения ликвидности самого пула — нет (ликвидность меняет хранилище, IVault.sol L441 PoolBalanceChanged,
/// лог от адреса хранилища). Ошибок нет.
pub fn liquidity_topics() -> Vec<B256> {
    Vec::new()
}

/// Применяет событие: известное событие пула → Refresh, иначе Ignored. Ошибок нет.
pub fn apply(e: &mut Entry, log: &Log) -> Applied {
    match log.topic0() {
        Some(t) if topics().contains(t) => Applied::Refresh(format!(
            "{}: событие пула Balancer V2 — перечитать состояние",
            e.family.title
        )),
        _ => Applied::Ignored,
    }
}

//! Файл: venues/bsc_dex/pools/events_balancer_v3.rs
//! Что делает: решение 441 (T433-F2) — события пула Balancer V3: topic0 для подписки и применение — известное событие
//! самого пула (AmpUpdateStarted, AmpUpdateStopped стейблсвапа) → перечитать пул целиком. Балансы, комиссию и паузу
//! пула меняет хранилище Vault V3 событиями от СВОЕГО адреса (Swap, LiquidityAdded, LiquidityRemoved,
//! SwapFeePercentageChanged, PoolPausedStateChanged — IVaultEvents.sol), курсы токенов меняют поставщики курса без событий
//! пула — подписка по адресу пула их не видит, поэтому вид ещё перечитывается каждым блоком (pools/per_block.rs).
//! Кто вызывает: pools/events.rs.
//! От чего зависит: abi_balancer_v3.rs, events.rs, registry.rs.
//! Настройки: нет.

use alloy::rpc::types::Log;
use alloy_primitives::B256;

use super::abi_balancer_v3;
use super::events::Applied;
use super::registry::Entry;

/// topic0 событий пула (abi_balancer_v3.rs; StablePool.sol L77, L83). Ошибок нет.
pub fn topics() -> Vec<B256> {
    abi_balancer_v3::topics()
}

/// topic0 событий изменения ликвидности самого пула — нет (ликвидность меняет хранилище: LiquidityAdded /
/// LiquidityRemoved от адреса Vault). Ошибок нет.
pub fn liquidity_topics() -> Vec<B256> {
    Vec::new()
}

/// Применяет событие: известное событие пула → Refresh, иначе Ignored. Ошибок нет.
pub fn apply(e: &mut Entry, log: &Log) -> Applied {
    match log.topic0() {
        Some(t) if topics().contains(t) => Applied::Refresh(format!(
            "{}: событие пула Balancer V3 — перечитать состояние",
            e.family.title
        )),
        _ => Applied::Ignored,
    }
}

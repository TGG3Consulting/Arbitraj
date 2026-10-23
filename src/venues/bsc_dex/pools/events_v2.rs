//! Файл: venues/bsc_dex/pools/events_v2.rs
//! Что делает: живое состояние пары V2 по событию Sync — новые резервы целиком.
//! Кто вызывает: pools/events.rs.
//! От чего зависит: pools/abi_v2v3.rs, registry.rs.
//! Настройки: нет.
//!
//! Sync(uint112 reserve0, uint112 reserve1) эмитится в _update после каждого mint/burn/swap/sync и содержит
//! новые резервы — https://github.com/Uniswap/v2-core/blob/6a9e7c97860676e0992f22a49665760444c1cdf5/contracts/UniswapV2Pair.sol#L73-L86
//! (PancakeSwap — то же, research/flow/bsc_dex.md, раздел 3: «достаточно подписки на Sync»).

use alloy::rpc::types::Log;
use alloy::sol_types::SolEvent;
use alloy_primitives::{B256, U256};

use crate::core::types::PoolState;

use super::abi_v2v3::IV2Pair;
use super::events::Applied;
use super::registry::Entry;

/// topic0 событий V2. Ошибок нет.
pub fn topics() -> Vec<B256> {
    vec![IV2Pair::Sync::SIGNATURE_HASH]
}

/// Применяет Sync. Ошибок нет: не разобрано → Refresh.
pub fn apply(e: &mut Entry, log: &Log) -> Applied {
    match IV2Pair::Sync::decode_log_data(&log.inner.data) {
        Ok(s) => {
            e.pool.state = PoolState::V2 {
                reserve0: U256::from(s.reserve0),
                reserve1: U256::from(s.reserve1),
            };
            Applied::Updated
        }
        Err(err) => Applied::Refresh(format!("Sync не по ABI: {err}")),
    }
}

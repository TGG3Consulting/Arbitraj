//! Файл: venues/bsc_dex/pools/events_solidly.rs
//! Что делает: живое состояние пула Solidly (ve(3,3)-форки: Topaz v2, THENA V1; решения 80, 85) по событию Sync —
//! новые резервы целиком (решения 83, 79: событие обновляет точечные переменные графа в ОЗУ). Флаг stable, масштабы
//! decimals и комиссия пула от Sync не меняются — они сохраняются из текущего состояния, обновляются только резервы.
//! Kто вызывает: pools/events.rs.
//! От чего зависит: pools/abi_solidly.rs, core/types (PoolState), registry.rs.
//! Настройки: нет.
//!
//! Sync(uint256 reserve0, uint256 reserve1) эмитится пулом Solidly после каждого mint/burn/swap/sync и несёт новые
//! резервы — velodrome-finance IPool.sol L29 (HEAD b3065d8, решение 23). У Solidly резервы uint256 (не uint112 как у
//! UniswapV2) — поэтому свой разбор, а не events_v2.rs. Пришёл Sync, а состояние не Solidly (рассинхрон вида) → Refresh.

use alloy::rpc::types::Log;
use alloy::sol_types::SolEvent;
use alloy_primitives::B256;

use crate::core::types::PoolState;

use super::abi_solidly::ISolidlyPool;
use super::events::Applied;
use super::registry::Entry;

/// topic0 событий Solidly (Sync). Ошибок нет.
pub fn topics() -> Vec<B256> {
    vec![ISolidlyPool::Sync::SIGNATURE_HASH]
}

/// topic0 добавления ликвидности — мелкие пулы (pools/depth.rs) слушают только его. У Solidly ликвидность меняет те же
/// резервы, что и обмен, и несёт их Sync (отдельного Mint для пробуждения достаточно Sync) — слушаем Sync. Ошибок нет.
pub fn liquidity_topics() -> Vec<B256> {
    vec![ISolidlyPool::Sync::SIGNATURE_HASH]
}

/// Применяет Sync: новые резервы в текущее состояние Solidly (stable/decimals/fee сохраняются). Ошибок нет: не
/// разобрано или состояние не Solidly → Refresh (перечитать пул целиком).
pub fn apply(e: &mut Entry, log: &Log) -> Applied {
    let s = match ISolidlyPool::Sync::decode_log_data(&log.inner.data) {
        Ok(s) => s,
        Err(err) => return Applied::Refresh(format!("Solidly Sync не по ABI: {err}")),
    };
    match &mut e.pool.state {
        PoolState::Solidly {
            reserve0, reserve1, ..
        } => {
            *reserve0 = s.reserve0;
            *reserve1 = s.reserve1;
            Applied::Updated
        }
        _ => Applied::Refresh("Solidly Sync: состояние пула не Solidly — перечитать".into()),
    }
}

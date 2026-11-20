//! Файл: venues/bsc_dex/pools/events_algebra_dir.rs
//! Что делает: решение 444 — живое состояние пула Algebra V1.9 с направленными комиссиями (Camelot V3, вид
//! algebra_dir_fee) по событиям. Swap/Mint/Burn — те же, что у THENA Fusion (events_algebra.rs: Swap — цена,
//! ликвидность, тик; Mint/Burn — перечитать пул). Своё — Fee(feeZto, feeOtz): событие несёт ОБЕ новые комиссии
//! целиком, поэтому они применяются сразу (без перечитывания) — Applied::Updated.
//! Порядок логов (settle.rs opens — Follow): Fee испускает только _updateFee (AlgebraPool.sol:549-558), а его зовут
//! обмен (AlgebraPool.sol:791, :793 — перед emit Swap :636 / :686) и изменение позиции с ненулевой ликвидностью
//! (AlgebraPool.sol:365 — перед emit Mint :497 / emit Burn :545) — за Fee в том же вызове всегда идёт ещё лог пула.
//! Кто вызывает: pools/events.rs.
//! От чего зависит: pools/abi_algebra_dir.rs, pools/events_algebra.rs, registry.rs.
//! Настройки: нет.
//!
//! Источник — cryptoalgebra/AlgebraV1.9, ветка direction-based-fee, коммит a45417ed5048c0a9ad3c673e41a00bb3c3880f6f:
//! src/core/contracts/interfaces/pool/IAlgebraPoolEvents.sol:26 (Mint), :58 (Burn), :70 (Swap), :107 (Fee).

use alloy::rpc::types::Log;
use alloy::sol_types::SolEvent;
use alloy_primitives::B256;

use crate::core::types::PoolState;

use super::abi_algebra_dir::IAlgebraDirPool;
use super::events::Applied;
use super::events_algebra;
use super::registry::Entry;

/// topic0 всех слушаемых событий пула: Swap/Mint/Burn (как V1.9) + Fee(uint16, uint16). Слияние с решением 463:
/// base_topics — без Fee(uint16) / Initialize THENA Fusion (набор подписок прежний, как у Integral). Ошибок нет.
pub fn topics() -> Vec<B256> {
    let mut t = events_algebra::base_topics();
    t.push(IAlgebraDirPool::Fee::SIGNATURE_HASH);
    t
}

/// topic0 добавления ликвидности — Mint, как у V1.9. Ошибок нет.
pub fn liquidity_topics() -> Vec<B256> {
    events_algebra::liquidity_topics()
}

/// Применяет событие (см. шапку). Принимает: запись пула, лог. Возвращает: итог применения. Ошибок нет: не разобрано
/// — Refresh.
pub fn apply(e: &mut Entry, log: &Log) -> Applied {
    if log.topic0().copied() != Some(IAlgebraDirPool::Fee::SIGNATURE_HASH) {
        return events_algebra::apply(e, log);
    }
    match IAlgebraDirPool::Fee::decode_log_data(&log.inner.data) {
        Ok(f) => fees(e, u32::from(f.feeZto), u32::from(f.feeOtz)),
        Err(err) => Applied::Refresh(format!("Fee (направленные комиссии) не по ABI: {err}")),
    }
}

/// Новые комиссии пула: token0 → token1 и token1 → token0 (пипсы). Ошибок нет: состояние не Algebra — Refresh.
fn fees(e: &mut Entry, zto: u32, otz: u32) -> Applied {
    match &mut e.pool.state {
        PoolState::Algebra { fee, fee_otz, .. } => {
            *fee = zto;
            *fee_otz = Some(otz);
            Applied::Updated
        }
        _ => Applied::Refresh("состояние пула не Algebra".into()),
    }
}

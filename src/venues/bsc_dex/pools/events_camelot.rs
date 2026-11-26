//! Файл: venues/bsc_dex/pools/events_camelot.rs
//! Что делает: решение 443 (T433-F4) — живое состояние пары Camelot V2 по её событиям: Sync — новые резервы (_update,
//! Pair.sol L129-L135), FeePercentUpdated — новые комиссии входа token0 / token1 (setFeePercent, L97-L104),
//! SetStableSwap — новый вид кривой (setStableSwap, L106-L118: событие несёт новое значение до присваивания, L115-L116).
//! Множители точности и прочее событием не меняются — сохраняются. Пришло событие, а состояние не Solidly с fee1
//! (рассинхрон вида) или данные не по ABI → Refresh (перечитать пару целиком).
//! Кто вызывает: pools/events.rs.
//! От чего зависит: abi_camelot.rs (topic0, ссылки на verified-исходник), math/camelot_v2.rs (MAX_FEE_PERCENT),
//! core/types (PoolState), events.rs, registry.rs.
//! Настройки: нет.

use alloy::rpc::types::Log;
use alloy::sol_types::SolEvent;
use alloy_primitives::{B256, U256};

use crate::core::types::PoolState;

use super::super::math::camelot_v2::MAX_FEE_PERCENT;
use super::abi_camelot::{self, ICamelotPair as P};
use super::events::Applied;
use super::registry::Entry;

/// Что меняет событие пары.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    /// Sync: резервы token0, token1.
    Reserves(U256, U256),
    /// FeePercentUpdated: комиссии входа token0, token1 (над FEE_DENOMINATOR).
    Fees(u16, u16),
    /// SetStableSwap: новый флаг stableSwap.
    Stable(bool),
}

/// topic0 всех событий пары, меняющих состояние (abi_camelot.rs). Ошибок нет.
pub fn topics() -> Vec<B256> {
    abi_camelot::topics()
}

/// topic0 добавления ликвидности (Mint). Ошибок нет.
pub fn liquidity_topics() -> Vec<B256> {
    abi_camelot::liquidity_topics()
}

/// Разбор события. Принимает: лог. Возвращает: Some(изменение) — событие состояния; None — иное событие. Ошибка —
/// текст (данные не по ABI; комиссия больше MAX_FEE_PERCENT или 0 — пара Camelot V2 так не позволяет, L99-L100).
pub fn parse(log: &Log) -> Result<Option<Change>, String> {
    let Some(t) = log.topic0() else {
        return Ok(None);
    };
    let data = &log.inner.data;
    if *t == P::Sync::SIGNATURE_HASH {
        let s = P::Sync::decode_log_data(data).map_err(|e| format!("Sync не по ABI: {e}"))?;
        return Ok(Some(Change::Reserves(
            U256::from(s.reserve0),
            U256::from(s.reserve1),
        )));
    }
    if *t == P::FeePercentUpdated::SIGNATURE_HASH {
        let s = P::FeePercentUpdated::decode_log_data(data)
            .map_err(|e| format!("FeePercentUpdated не по ABI: {e}"))?;
        let max = u32::from(s.token0FeePercent.max(s.token1FeePercent));
        if max > MAX_FEE_PERCENT {
            return Err(format!(
                "FeePercentUpdated: комиссия {max} > MAX_FEE_PERCENT"
            ));
        }
        // L100: setFeePercent требует обе комиссии > 0.
        if s.token0FeePercent == 0 || s.token1FeePercent == 0 {
            return Err("FeePercentUpdated: комиссия 0 (пара так не позволяет, L100)".into());
        }
        return Ok(Some(Change::Fees(s.token0FeePercent, s.token1FeePercent)));
    }
    if *t == P::SetStableSwap::SIGNATURE_HASH {
        let s = P::SetStableSwap::decode_log_data(data)
            .map_err(|e| format!("SetStableSwap не по ABI: {e}"))?;
        return Ok(Some(Change::Stable(s.stableSwap)));
    }
    Ok(None)
}

/// Применяет событие к записи. Ошибок нет: не разобрано или состояние не пары Camelot → Refresh; иное событие —
/// Ignored.
pub fn apply(e: &mut Entry, log: &Log) -> Applied {
    let change = match parse(log) {
        Ok(Some(c)) => c,
        Ok(None) => return Applied::Ignored,
        Err(w) => return Applied::Refresh(format!("Camelot V2: {w}")),
    };
    let PoolState::Solidly {
        reserve0,
        reserve1,
        stable,
        fee,
        fee1: Some(fee1),
        ..
    } = &mut e.pool.state
    else {
        return Applied::Refresh("Camelot V2: состояние пары не то — перечитать".into());
    };
    match change {
        Change::Reserves(r0, r1) => {
            *reserve0 = r0;
            *reserve1 = r1;
        }
        Change::Fees(f0, f1) => {
            fee.value = u64::from(f0);
            fee1.value = u64::from(f1);
        }
        Change::Stable(s) => *stable = s,
    }
    Applied::Updated
}

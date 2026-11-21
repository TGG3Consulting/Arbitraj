//! Файл: venues/bsc_dex/pools/events_basin.rs
//! Что делает: события скважины Basin (решение 485) — topic0 для подписки и применение к состоянию:
//! - Swap(fromToken, toToken, amountIn, amountOut, recipient) — резерв fromToken += amountIn, резерв toToken −=
//!   amountOut, число LP не меняется (swapFrom L247-L260, swapTo L301-L314, swapFromFeeOnTransfer — amountIn уже
//!   фактически пришедший, L232);
//! - Shift(reserves, toToken, amountOut, recipient) — резервы после сдвига целиком из события (L395-L405), LP те же;
//! - Sync, AddLiquidity, RemoveLiquidity*, Upgraded прокси — перечитать скважину целиком: первые меняют число
//!   LP-токенов, Upgraded — реализацию прокси; полное чтение (load_basin.rs) заново читает слот реализации ERC-1967
//!   (eth_getStorageAt), сверяет его с Aquifer (dex.basin_well_impls) и функцию скважины со списками — не прошло,
//!   пул недостоверен (ошибка чтения с причиной).
//!
//! Событие не сходится (токен не из скважины, резерв ушёл бы ниже нуля, массив не из двух резервов) — перечитать.
//! Кто вызывает: pools/events.rs.
//! От чего зависит: abi_basin.rs, events.rs, registry.rs, core/types (BasinWellData).
//! Настройки: нет.
//!
//! Источник: BeanstalkFarms/Basin@ecf69233 src/Well.sol (строки выше), src/interfaces/IWell.sol L31-L77.

use alloy::rpc::types::Log;
use alloy::sol_types::SolEvent;
use alloy_primitives::{Address, B256, U256};

use crate::core::types::{BasinWellData, PoolState};

use super::abi_basin::{self, IBasinWell as W};
use super::events::Applied;
use super::registry::Entry;

/// topic0 всех слушаемых событий скважины (abi_basin.rs). Ошибок нет.
pub fn topics() -> Vec<B256> {
    abi_basin::topics()
}

/// topic0 событий изменения ликвидности скважины. Ошибок нет.
pub fn liquidity_topics() -> Vec<B256> {
    abi_basin::liquidity_topics()
}

/// Применяет событие. Возвращает: Updated (Swap / Shift применены), Refresh (прочие события скважины, не сошлось),
/// Ignored (не событие скважины). Ошибок нет.
pub fn apply(e: &mut Entry, log: &Log) -> Applied {
    let title = e.family.title;
    let refresh =
        |why: &str| Applied::Refresh(format!("{title}: скважина Basin — {why}, перечитать"));
    let Some(t) = log.topic0().copied() else {
        return Applied::Ignored;
    };
    if !topics().contains(&t) {
        return Applied::Ignored;
    }
    let PoolState::BasinWell(d) = &mut e.pool.state else {
        return refresh("состояние пула не BasinWell");
    };
    let done = if t == W::Swap::SIGNATURE_HASH {
        match W::Swap::decode_log_data(&log.inner.data) {
            Ok(s) => swap(d, s.fromToken, s.toToken, s.amountIn, s.amountOut),
            Err(_) => None,
        }
    } else if t == W::Shift::SIGNATURE_HASH {
        match W::Shift::decode_log_data(&log.inner.data) {
            Ok(s) if s.reserves.len() == 2 => {
                d.reserve0 = s.reserves[0];
                d.reserve1 = s.reserves[1];
                Some(())
            }
            _ => None,
        }
    } else {
        return refresh("событие ликвидности, Sync или смена реализации");
    };
    match done {
        Some(()) => Applied::Updated,
        None => refresh("событие Swap / Shift не сходится с состоянием"),
    }
}

/// Swap: резерв входа += amountIn, резерв выхода −= amountOut. None — токены не из скважины или одинаковы,
/// переполнение / ниже нуля. Ошибок нет.
pub fn swap(
    d: &mut BasinWellData,
    from: Address,
    to: Address,
    amount_in: U256,
    amount_out: U256,
) -> Option<()> {
    if from == to || !d.has(from) || !d.has(to) {
        return None;
    }
    let (r_in, r_out) = if from == d.token0 {
        (d.reserve0, d.reserve1)
    } else {
        (d.reserve1, d.reserve0)
    };
    let r_in = r_in.checked_add(amount_in)?;
    let r_out = r_out.checked_sub(amount_out)?;
    if from == d.token0 {
        (d.reserve0, d.reserve1) = (r_in, r_out);
    } else {
        (d.reserve1, d.reserve0) = (r_in, r_out);
    }
    Some(())
}

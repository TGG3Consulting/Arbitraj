//! Файл: venues/bsc_dex/pools/events_v3.rs
//! Что делает: живое состояние пула V3: Swap — цена, ликвидность и тик после обмена; Mint/Burn — карта тиков
//! и текущая ликвидность, если текущий тик внутри диапазона позиции.
//! Кто вызывает: pools/events.rs.
//! От чего зависит: pools/abi_v2v3.rs, registry.rs, ticks.rs.
//! Настройки: нет.
//!
//! Swap эмитится в конце swap со значениями после обмена — PancakeV3Pool.sol#L817 (pancake-v3-contracts 98684794);
//! Swap PancakeSwap — 9 полей, Uniswap — 7 полей (разные topic0, research/flow/bsc_dex.md, раздел 2–3).
//! Mint/Burn: amount — ликвидность позиции; liquidity пула меняется, если tickLower ≤ tick < tickUpper —
//! https://github.com/Uniswap/v3-core/blob/5ea68a77cb541c6b3812df783be71ba8c081e25f/contracts/UniswapV3Pool.sol#L328-L366 .

use alloy::rpc::types::Log;
use alloy::sol_types::SolEvent;
use alloy_primitives::{B256, U256};

use crate::core::types::PoolState;

use super::abi_v2v3::{IPancakeV3Pool, IUniswapV3Pool, IV3Pool};
use super::events::Applied;
use super::families::Proto;
use super::registry::Entry;

/// topic0 событий V3 своей DEX. Ошибок нет.
pub fn topics(p: Proto) -> Vec<B256> {
    let swap = match p {
        Proto::PancakeV3 => IPancakeV3Pool::Swap::SIGNATURE_HASH,
        _ => IUniswapV3Pool::Swap::SIGNATURE_HASH,
    };
    vec![
        swap,
        IV3Pool::Mint::SIGNATURE_HASH,
        IV3Pool::Burn::SIGNATURE_HASH,
    ]
}

/// Применяет Swap / Mint / Burn. Ошибок нет: расхождение или не разобрано → Refresh.
pub fn apply(e: &mut Entry, log: &Log) -> Applied {
    let Some(t0) = log.topic0().copied() else {
        return Applied::Ignored;
    };
    let d = &log.inner.data;
    let swap = |sqrt: U256, liquidity: u128, tick: i32, e: &mut Entry| {
        e.pool.state = PoolState::V3 {
            sqrt_price_x96: sqrt,
            liquidity,
            tick,
        };
        Applied::Updated
    };
    if t0 == IPancakeV3Pool::Swap::SIGNATURE_HASH && e.family.proto == Proto::PancakeV3 {
        return match IPancakeV3Pool::Swap::decode_log_data(d) {
            Ok(s) => swap(U256::from(s.sqrtPriceX96), s.liquidity, s.tick.as_i32(), e),
            Err(err) => Applied::Refresh(format!("Swap не по ABI: {err}")),
        };
    }
    // Slipstream (Topaz v3) — форк Uni V3: тот же topic0 и 7 полей Swap, что у Uniswap V3.
    if t0 == IUniswapV3Pool::Swap::SIGNATURE_HASH
        && matches!(e.family.proto, Proto::UniswapV3 | Proto::Slipstream)
    {
        return match IUniswapV3Pool::Swap::decode_log_data(d) {
            Ok(s) => swap(U256::from(s.sqrtPriceX96), s.liquidity, s.tick.as_i32(), e),
            Err(err) => Applied::Refresh(format!("Swap не по ABI: {err}")),
        };
    }
    let change = if t0 == IV3Pool::Mint::SIGNATURE_HASH {
        IV3Pool::Mint::decode_log_data(d)
            .map(|m| (m.tickLower.as_i32(), m.tickUpper.as_i32(), m.amount as i128))
    } else if t0 == IV3Pool::Burn::SIGNATURE_HASH {
        IV3Pool::Burn::decode_log_data(d).map(|b| {
            (
                b.tickLower.as_i32(),
                b.tickUpper.as_i32(),
                -(b.amount as i128),
            )
        })
    } else {
        return Applied::Ignored;
    };
    match change {
        Ok((lower, upper, delta)) => modify(e, lower, upper, delta),
        Err(err) => Applied::Refresh(format!("Mint/Burn не по ABI: {err}")),
    }
}

/// Изменение позиции: карта тиков и, если тик внутри, текущая ликвидность (общая логика V3 и CL).
/// Принимает: запись, границы, Δ ликвидности. Возвращает: Updated или Refresh при расхождении.
pub fn modify(e: &mut Entry, lower: i32, upper: i32, delta: i128) -> Applied {
    if let Some(t) = e.ticks.as_mut()
        && !t.modify(lower, upper, delta)
    {
        return Applied::Refresh(format!(
            "позиция [{lower}, {upper}) Δ{delta} не сходится с картой тиков"
        ));
    }
    let (tick, liq) = match &mut e.pool.state {
        PoolState::V3 {
            tick, liquidity, ..
        }
        | PoolState::Cl {
            tick, liquidity, ..
        } => (*tick, liquidity),
        _ => return Applied::Refresh("состояние пула не V3/CL".into()),
    };
    if lower <= tick && tick < upper {
        let new = if delta >= 0 {
            liq.checked_add(delta.unsigned_abs())
        } else {
            liq.checked_sub(delta.unsigned_abs())
        };
        match new {
            Some(v) => *liq = v,
            None => {
                return Applied::Refresh(format!("ликвидность {liq} Δ{delta} уходит за предел"));
            }
        }
    }
    Applied::Updated
}

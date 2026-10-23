//! Файл: venues/bsc_dex/pools/events_cl.rs
//! Что делает: живое состояние пула CL (Infinity CL, Uniswap V4): Swap — цена, ликвидность, тик после обмена;
//! ModifyLiquidity — карта тиков и текущая ликвидность; изменение комиссий — DynamicLPFeeUpdated (Infinity),
//! ProtocolFeeUpdated (оба). Donate цену не меняет — не слушается.
//! Кто вызывает: pools/events.rs.
//! От чего зависит: pools/abi_v4.rs, events_v3.rs (modify), registry.rs.
//! Настройки: нет.
//!
//! События и поля — research/flow/bsc_v4.md, раздел 4: Swap несёт состояние после обмена (ICLPoolManager.sol#L59-L79,
//! Uniswap IPoolManager.sol#L82-L100); ModifyLiquidity(tickLower, tickUpper, liquidityDelta) — как Mint/Burn V3;
//! у Uniswap динамическая комиссия меняется без события (PoolManager.sol#L339-L346) — такие пулы группы B
//! проверяются квотером (шаг 1c), а не по lp_fee из состояния.

use alloy::rpc::types::Log;
use alloy::sol_types::SolEvent;
use alloy_primitives::{B256, U256};

use crate::core::types::PoolState;

use super::abi_v4::{IInfinityCl, IInfinityFees, IUniswapV4};
use super::events::Applied;
use super::events_v3::modify;
use super::families::Proto;
use super::registry::Entry;

/// topic0 событий CL своей DEX. Ошибок нет.
pub fn topics(p: Proto) -> Vec<B256> {
    match p {
        Proto::InfinityCl => vec![
            IInfinityCl::Swap::SIGNATURE_HASH,
            IInfinityCl::ModifyLiquidity::SIGNATURE_HASH,
            IInfinityFees::DynamicLPFeeUpdated::SIGNATURE_HASH,
            IInfinityFees::ProtocolFeeUpdated::SIGNATURE_HASH,
        ],
        _ => vec![
            IUniswapV4::Swap::SIGNATURE_HASH,
            IUniswapV4::ModifyLiquidity::SIGNATURE_HASH,
            IUniswapV4::ProtocolFeeUpdated::SIGNATURE_HASH,
        ],
    }
}

/// Применяет событие CL. Ошибок нет: не разобрано или расхождение → Refresh.
pub fn apply(e: &mut Entry, log: &Log) -> Applied {
    let Some(t0) = log.topic0().copied() else {
        return Applied::Ignored;
    };
    let d = &log.inner.data;
    let bad = |what: &str, err: alloy::sol_types::Error| {
        Applied::Refresh(format!("{what} не по ABI: {err}"))
    };
    if t0 == IInfinityCl::Swap::SIGNATURE_HASH {
        return match IInfinityCl::Swap::decode_log_data(d) {
            Ok(s) => swap(e, U256::from(s.sqrtPriceX96), s.liquidity, s.tick.as_i32()),
            Err(err) => bad("Swap", err),
        };
    }
    if t0 == IUniswapV4::Swap::SIGNATURE_HASH {
        return match IUniswapV4::Swap::decode_log_data(d) {
            Ok(s) => swap(e, U256::from(s.sqrtPriceX96), s.liquidity, s.tick.as_i32()),
            Err(err) => bad("Swap", err),
        };
    }
    // ModifyLiquidity у Infinity CL и Uniswap V4 — одна сигнатура (bsc_v4.md, раздел 4).
    if t0 == IInfinityCl::ModifyLiquidity::SIGNATURE_HASH {
        return match IInfinityCl::ModifyLiquidity::decode_log_data(d) {
            Ok(m) => match i128::try_from(m.liquidityDelta) {
                Ok(delta) => modify(e, m.tickLower.as_i32(), m.tickUpper.as_i32(), delta),
                Err(_) => Applied::Refresh("liquidityDelta вне int128".into()),
            },
            Err(err) => bad("ModifyLiquidity", err),
        };
    }
    if t0 == IInfinityFees::DynamicLPFeeUpdated::SIGNATURE_HASH {
        return match IInfinityFees::DynamicLPFeeUpdated::decode_log_data(d) {
            Ok(f) => set_fee(e, None, Some(f.dynamicLPFee.to())),
            Err(err) => bad("DynamicLPFeeUpdated", err),
        };
    }
    if t0 == IInfinityFees::ProtocolFeeUpdated::SIGNATURE_HASH {
        return match IInfinityFees::ProtocolFeeUpdated::decode_log_data(d) {
            Ok(f) => set_fee(e, Some(f.protocolFee.to()), None),
            Err(err) => bad("ProtocolFeeUpdated", err),
        };
    }
    Applied::Ignored
}

/// Состояние после обмена (комиссии остаются прежними).
fn swap(e: &mut Entry, sqrt: U256, liq: u128, t: i32) -> Applied {
    match &mut e.pool.state {
        PoolState::Cl {
            sqrt_price_x96,
            tick,
            liquidity,
            ..
        } => {
            *sqrt_price_x96 = sqrt;
            *tick = t;
            *liquidity = liq;
            Applied::Updated
        }
        _ => Applied::Refresh("состояние пула не CL".into()),
    }
}

/// Новая протокольная и/или LP-комиссия (единицы 1e-6).
fn set_fee(e: &mut Entry, protocol: Option<u32>, lp: Option<u32>) -> Applied {
    match &mut e.pool.state {
        PoolState::Cl {
            protocol_fee,
            lp_fee,
            ..
        }
        | PoolState::Bin {
            protocol_fee,
            lp_fee,
            ..
        } => {
            if let Some(p) = protocol {
                *protocol_fee = p;
            }
            if let Some(l) = lp {
                *lp_fee = l;
            }
            Applied::Updated
        }
        _ => Applied::Refresh("состояние пула не V4".into()),
    }
}

/// Для events_bin: те же события комиссий. Принимает: запись, лог. Возвращает: Some — это событие комиссии.
pub fn fee_event(e: &mut Entry, log: &Log) -> Option<Applied> {
    let t0 = log.topic0().copied()?;
    if t0 == IInfinityFees::DynamicLPFeeUpdated::SIGNATURE_HASH
        || t0 == IInfinityFees::ProtocolFeeUpdated::SIGNATURE_HASH
    {
        return Some(apply(e, log));
    }
    None
}

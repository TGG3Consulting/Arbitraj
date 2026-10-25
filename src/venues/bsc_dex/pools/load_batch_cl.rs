//! Файл: venues/bsc_dex/pools/load_batch_cl.rs
//! Что делает: части пачечного чтения (load_batch.rs) для пулов CL — PancakeSwap Infinity CL (геттеры у менеджера) и
//! Uniswap V4 (StateView): вызовы и разбор slot0 + liquidity, слов битмапа и данных установленных тиков. Те же функции и
//! правила, что у чтения по одному (load_cl.rs). Ступень 3 — load_batch_ticks.rs.
//! Кто вызывает: pools/load_batch.rs.
//! От чего зависит: pools/abi_v4.rs, node/call.rs (decode), load_cl.rs (set_ticks, i24), load_batch.rs (Want).
//! Настройки: dex.state_view [uniswap_v4].
//!
//! Infinity: getSlot0, getLiquidity, getPoolBitmapInfo, getPoolTickInfo — ICLPoolManager.sol#L89-L108;
//! Uniswap: StateView getSlot0, getLiquidity, getTickBitmap, getTickLiquidity — StateView.sol#L19-L77
//! (research/flow/bsc_v4.md, раздел 4; как в load_cl.rs).

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, B256, Bytes, U256};

use crate::core::types::{PoolRef, PoolState};

use super::super::ctx::Ctx;
use super::super::node::call;
use super::abi_v4::{IInfinityCl, IStateView};
use super::families::Proto;
use super::load_batch::Want;
use super::load_batch::{Miss, miss};
use super::load_batch_v2v3::PoolStateTick;
use super::load_cl;

/// Кто отвечает на геттеры и PoolId. Ошибки: причина (не CL, нет dex.state_view, ссылка не того вида).
pub fn target(ctx: &Ctx, w: &Want) -> Result<(bool, Address, B256), Miss> {
    let PoolRef::Singleton { manager, id } = w.reference else {
        return Err(miss(format!(
            "{}: ссылка на пул не того вида",
            w.family.title
        )));
    };
    match w.family.proto {
        Proto::InfinityCl => Ok((true, *manager, *id)),
        Proto::UniswapV4 => {
            let sv = ctx.dex_addr("dex.state_view", w.family.id)?;
            Ok((false, sv, *id))
        }
        _ => Err(miss(format!("{}: не CL", w.family.title))),
    }
}

/// Вызовы slot0 и liquidity. Ошибки: как у target.
pub fn head(ctx: &Ctx, w: &Want) -> Result<Vec<(Address, Bytes)>, Miss> {
    let (inf, to, id) = target(ctx, w)?;
    let (a, b) = if inf {
        (
            IInfinityCl::getSlot0Call { id }.abi_encode(),
            IInfinityCl::getLiquidityCall { id }.abi_encode(),
        )
    } else {
        (
            IStateView::getSlot0Call { poolId: id }.abi_encode(),
            IStateView::getLiquidityCall { poolId: id }.abi_encode(),
        )
    };
    Ok(vec![(to, Bytes::from(a)), (to, Bytes::from(b))])
}

/// Разбор slot0 и liquidity. Ошибки: причина (ответ не по ABI).
pub fn parse_head(w: &Want, b: &[Bytes]) -> Result<PoolStateTick, Miss> {
    let m = |e: Miss| e;
    let (sqrt, tick, pf, lf, liquidity) = if w.family.proto == Proto::InfinityCl {
        let s = call::decode::<IInfinityCl::getSlot0Call>(&b[0]).map_err(m)?;
        let l = call::decode::<IInfinityCl::getLiquidityCall>(&b[1]).map_err(m)?;
        (s.sqrtPriceX96, s.tick, s.protocolFee, s.lpFee, l)
    } else {
        let s = call::decode::<IStateView::getSlot0Call>(&b[0]).map_err(m)?;
        let l = call::decode::<IStateView::getLiquidityCall>(&b[1]).map_err(m)?;
        (s.sqrtPriceX96, s.tick, s.protocolFee, s.lpFee, l)
    };
    let tick = tick.as_i32();
    let state = PoolState::Cl {
        sqrt_price_x96: U256::from(sqrt),
        tick,
        liquidity,
        protocol_fee: pf.to(),
        lp_fee: lf.to(),
    };
    Ok(PoolStateTick::Ticked(state, tick))
}

/// Вызовы слов битмапа [lo, hi]. Ошибки: как у target.
pub fn word_calls(
    ctx: &Ctx,
    w: &Want,
    (lo, hi): (i16, i16),
) -> Result<Vec<(Address, Bytes)>, Miss> {
    let (inf, to, id) = target(ctx, w)?;
    Ok((lo..=hi)
        .map(|word| {
            let d = if inf {
                IInfinityCl::getPoolBitmapInfoCall { id, word }.abi_encode()
            } else {
                IStateView::getTickBitmapCall {
                    poolId: id,
                    tick: word,
                }
                .abi_encode()
            };
            (to, Bytes::from(d))
        })
        .collect())
}

/// Установленные тики по словам битмапа. Ошибки: причина (ответ не по ABI).
pub fn set_ticks(w: &Want, first: i16, ans: &[Bytes], spacing: i32) -> Result<Vec<i32>, Miss> {
    let m = |e: Miss| e;
    let words = ans
        .iter()
        .map(|b| {
            if w.family.proto == Proto::InfinityCl {
                call::decode::<IInfinityCl::getPoolBitmapInfoCall>(b).map_err(m)
            } else {
                call::decode::<IStateView::getTickBitmapCall>(b).map_err(m)
            }
        })
        .collect::<Result<Vec<U256>, Miss>>()?;
    Ok(load_cl::set_ticks(first, &words, spacing))
}

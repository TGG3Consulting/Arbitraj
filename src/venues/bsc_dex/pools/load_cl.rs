//! Файл: venues/bsc_dex/pools/load_cl.rs
//! Что делает: чтение состояния пула CL (PancakeSwap Infinity CL, Uniswap V4) на заданном блоке: slot0
//! (цена, тик, протокольная и LP-комиссия), liquidity и тики по битмапу в словах вокруг текущего тика.
//! Кто вызывает: pools/load.rs.
//! От чего зависит: ctx.rs, node/call.rs, pools/abi_v4.rs, ticks.rs, load_v2v3.rs (word_range).
//! Настройки: dex.tick_words [bsc_dex]; dex.state_view [uniswap_v4] (для Uniswap; у Infinity геттеры у менеджера).
//!
//! Infinity: getSlot0, getLiquidity, getPoolBitmapInfo, getPoolTickInfo — ICLPoolManager.sol#L89-L108;
//! Uniswap: StateView getSlot0, getLiquidity, getTickBitmap, getTickLiquidity — StateView.sol#L19-L77
//! (research/flow/bsc_v4.md, раздел 4 и схема п. 5: TickLens для V4 в BSC не найден — читаем битмап).
//! Тик из бита: tick = (word · 256 + bit) · tickSpacing (TickBitmap.position, обратное преобразование).

use std::collections::BTreeMap;

use alloy_primitives::aliases::I24;
use alloy_primitives::{Address, B256, U256};

use crate::core::types::PoolState;
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::node::call;
use super::abi_v4::{IInfinityCl, IStateView};
use super::families::{Family, Proto};
use super::load_v2v3::word_range;
use super::ticks::{TickData, TickMap};

/// Состояние и карта тиков пула CL.
/// Принимает: вид пула, менеджер, PoolId, tickSpacing, блок.
/// Ошибки: NotConfigured (dex.tick_words, dex.state_view), узел, ответ не по ABI.
pub async fn load(
    ctx: &Ctx,
    f: &Family,
    manager: Address,
    id: B256,
    spacing: i32,
    tag: &str,
) -> GatewayResult<(PoolState, TickMap)> {
    let infinity = match f.proto {
        Proto::InfinityCl => true,
        Proto::UniswapV4 => false,
        _ => return Err(ctx.err(ErrorCode::Internal, format!("{}: не CL", f.title))),
    };
    let reader = if infinity {
        manager
    } else {
        ctx.dex_addr("dex.state_view", f.id)?
    };
    let (sqrt, tick, protocol_fee, lp_fee, liquidity) = if infinity {
        let s = call::view(ctx, reader, &IInfinityCl::getSlot0Call { id }, tag).await?;
        let l = call::view(ctx, reader, &IInfinityCl::getLiquidityCall { id }, tag).await?;
        (s.sqrtPriceX96, s.tick, s.protocolFee, s.lpFee, l)
    } else {
        let s = call::view(ctx, reader, &IStateView::getSlot0Call { poolId: id }, tag).await?;
        let l = call::view(
            ctx,
            reader,
            &IStateView::getLiquidityCall { poolId: id },
            tag,
        )
        .await?;
        (s.sqrtPriceX96, s.tick, s.protocolFee, s.lpFee, l)
    };
    let tick = tick.as_i32();
    let (lo, hi) = word_range(ctx, tick, spacing)?;
    let ticks = if infinity {
        let calls: Vec<_> = (lo..=hi)
            .map(|w| IInfinityCl::getPoolBitmapInfoCall { id, word: w })
            .collect();
        let words = call::view_many(ctx, reader, &calls, tag).await?;
        let found = set_ticks(lo, &words, spacing);
        let calls = found
            .iter()
            .map(|t| {
                Ok(IInfinityCl::getPoolTickInfoCall {
                    id,
                    tick: i24(ctx, *t)?,
                })
            })
            .collect::<GatewayResult<Vec<_>>>()?;
        let infos = call::view_many(ctx, reader, &calls, tag).await?;
        found
            .iter()
            .zip(infos)
            .map(|(t, i)| {
                (
                    *t,
                    TickData {
                        gross: i.liquidityGross,
                        net: i.liquidityNet,
                    },
                )
            })
            .collect()
    } else {
        let calls: Vec<_> = (lo..=hi)
            .map(|w| IStateView::getTickBitmapCall {
                poolId: id,
                tick: w,
            })
            .collect();
        let words = call::view_many(ctx, reader, &calls, tag).await?;
        let found = set_ticks(lo, &words, spacing);
        let calls = found
            .iter()
            .map(|t| {
                Ok(IStateView::getTickLiquidityCall {
                    poolId: id,
                    tick: i24(ctx, *t)?,
                })
            })
            .collect::<GatewayResult<Vec<_>>>()?;
        let infos = call::view_many(ctx, reader, &calls, tag).await?;
        found
            .iter()
            .zip(infos)
            .map(|(t, i)| {
                (
                    *t,
                    TickData {
                        gross: i.liquidityGross,
                        net: i.liquidityNet,
                    },
                )
            })
            .collect()
    };
    let state = PoolState::Cl {
        sqrt_price_x96: U256::from(sqrt),
        tick,
        liquidity,
        protocol_fee: protocol_fee.to(),
        lp_fee: lp_fee.to(),
    };
    Ok((
        state,
        TickMap {
            spacing,
            ticks: ticks_map(ticks),
            words: (lo, hi),
        },
    ))
}

/// Тики по установленным битам слов битмапа. Принимает: первое слово, слова, tickSpacing. Ошибок нет.
pub fn set_ticks(first: i16, words: &[U256], spacing: i32) -> Vec<i32> {
    let mut out = Vec::new();
    for (i, w) in words.iter().enumerate() {
        let word = i32::from(first) + i as i32;
        for bit in 0..256usize {
            if w.bit(bit) {
                out.push((word * 256 + bit as i32) * spacing);
            }
        }
    }
    out
}

/// int24 для ABI. Ошибки: BadResponse — тик вне int24 (по TickMath так не бывает: значит, данные битмапа неверны).
pub fn i24(ctx: &Ctx, t: i32) -> GatewayResult<I24> {
    I24::try_from(t).map_err(|_| ctx.err(ErrorCode::BadResponse, format!("тик {t} вне int24")))
}

/// Список (тик, данные) → карта. Ошибок нет.
fn ticks_map(v: Vec<(i32, TickData)>) -> BTreeMap<i32, TickData> {
    v.into_iter().collect()
}

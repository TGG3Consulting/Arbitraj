//! Файл: venues/bsc_dex/pools/load_v2v3.rs
//! Что делает: чтение состояния пулов V2 и V3 на заданном блоке: V2 — резервы; V3 — slot0, liquidity и
//! инициализированные тики через TickLens в словах битмапа вокруг текущего тика.
//! Кто вызывает: pools/load.rs.
//! От чего зависит: ctx.rs, node/call.rs, pools/abi_v2v3.rs, ticks.rs, discover.rs (Found).
//! Настройки: dex.tick_words [bsc_dex] (сколько слов в каждую сторону), dex.tick_lens [вид пула V3].
//!
//! getReserves / slot0 / liquidity / TickLens.getPopulatedTicksInWord — research/flow/bsc_dex.md, раздел 3
//! (IPancakeV3PoolState.sol#L21-L48, IUniswapV3PoolState.sol#L21-L32, ITickLens.sol#L6-L24).
//! TickLens: «avoids the waterfall of fetching the tick bitmap…» — один вызов на слово.

use std::collections::BTreeMap;

use alloy_primitives::{Address, U256};

use crate::core::types::PoolState;
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::node::call;
use super::abi_v2v3::{IPancakeV3Pool, ITickLens, IUniswapV3Pool, IV2Pair, IV3Pool};
use super::families::{Family, Proto};
use super::ticks::{TickData, TickMap};

/// Резервы пары V2 на блоке. Ошибки: узел, ответ не по ABI.
pub async fn v2(ctx: &Ctx, pair: Address, tag: &str) -> GatewayResult<PoolState> {
    let r = call::view(ctx, pair, &IV2Pair::getReservesCall {}, tag).await?;
    Ok(PoolState::V2 {
        reserve0: U256::from(r.reserve0),
        reserve1: U256::from(r.reserve1),
    })
}

/// Состояние пула V3 и карта тиков на блоке.
/// Принимает: вид пула, адрес пула, tickSpacing (из PoolCreated), блок.
/// Ошибки: NotConfigured (dex.tick_words, dex.tick_lens), узел, ответ не по ABI.
pub async fn v3(
    ctx: &Ctx,
    f: &Family,
    pool: Address,
    spacing: i32,
    tag: &str,
) -> GatewayResult<(PoolState, TickMap)> {
    let (sqrt, tick) = match f.proto {
        Proto::PancakeV3 => {
            let s = call::view(ctx, pool, &IPancakeV3Pool::slot0Call {}, tag).await?;
            (U256::from(s.sqrtPriceX96), s.tick.as_i32())
        }
        Proto::UniswapV3 => {
            let s = call::view(ctx, pool, &IUniswapV3Pool::slot0Call {}, tag).await?;
            (U256::from(s.sqrtPriceX96), s.tick.as_i32())
        }
        _ => return Err(ctx.err(ErrorCode::Internal, format!("{}: не V3", f.title))),
    };
    let liquidity = call::view(ctx, pool, &IV3Pool::liquidityCall {}, tag).await?;
    let ticks = v3_ticks(ctx, f, pool, spacing, tick, tag).await?;
    let state = PoolState::V3 {
        sqrt_price_x96: sqrt,
        liquidity,
        tick,
    };
    Ok((state, ticks))
}

/// Тики V3 через TickLens по словам [w − n, w + n], n = dex.tick_words.
async fn v3_ticks(
    ctx: &Ctx,
    f: &Family,
    pool: Address,
    spacing: i32,
    tick: i32,
    tag: &str,
) -> GatewayResult<TickMap> {
    let lens = ctx.dex_addr("dex.tick_lens", f.id)?;
    let (lo, hi) = word_range(ctx, tick, spacing)?;
    let calls: Vec<_> = (lo..=hi)
        .map(|w| ITickLens::getPopulatedTicksInWordCall {
            pool,
            tickBitmapIndex: w,
        })
        .collect();
    let mut ticks = BTreeMap::new();
    for word in call::view_many(ctx, lens, &calls, tag).await? {
        for t in word {
            ticks.insert(
                t.tick.as_i32(),
                TickData {
                    gross: t.liquidityGross,
                    net: t.liquidityNet,
                },
            );
        }
    }
    Ok(TickMap {
        spacing,
        ticks,
        words: (lo, hi),
    })
}

/// Слова битмапа вокруг текущего тика: [w − n, w + n] в пределах int16.
/// Ошибки: NotConfigured (dex.tick_words), Internal (tickSpacing ≤ 0).
pub fn word_range(ctx: &Ctx, tick: i32, spacing: i32) -> GatewayResult<(i16, i16)> {
    if spacing <= 0 {
        return Err(ctx.err(ErrorCode::BadResponse, format!("tickSpacing {spacing} ≤ 0")));
    }
    let n = ctx.int("dex.tick_words")?.min(i16::MAX as u64) as i32;
    let w = i32::from(TickMap::word_of(tick, spacing));
    let lo = (w - n).max(i32::from(i16::MIN)) as i16;
    let hi = (w + n).min(i32::from(i16::MAX)) as i16;
    Ok((lo, hi))
}

//! Файл: venues/bsc_dex/pools/load_v2v3.rs
//! Что делает: чтение состояния пулов V2 и V3 на заданном блоке: V2 — резервы; V3 — slot0, liquidity и
//! инициализированные тики через TickLens в словах битмапа вокруг текущего тика.
//! Кто вызывает: pools/load.rs.
//! От чего зависит: ctx.rs, node/call.rs, pools/abi_v2v3.rs, ticks.rs, discover.rs (Found).
//! Настройки: dex.tick_words, dex.depth_band_pct [bsc_dex] (сколько слов в каждую сторону — больше из двух, решение 73),
//! dex.tick_lens [вид пула V3].
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

/// Сколько слов битмапа в каждую сторону покрывают полосу `pct` % при шаге `spacing`: тиков полосы (большая из сторон,
/// quote/depth_band.rs band_ticks) / (256 · spacing) вверх до целого, плюс слово текущего тика. Полоса вне (0, 100) —
/// 0. Ошибок нет.
pub fn band_words(pct: rust_decimal::Decimal, spacing: i32) -> i32 {
    use super::super::quote::depth_band::band_ticks;
    use rust_decimal::prelude::ToPrimitive;
    let b = (pct / rust_decimal::Decimal::ONE_HUNDRED)
        .to_f64()
        .unwrap_or(0.0);
    let (Some(up), Some(down)) = (band_ticks(b, true), band_ticks(b, false)) else {
        return 0;
    };
    let per_word = 256i64 * i64::from(spacing.max(1));
    let words = (i64::from(up.max(down)) + per_word - 1) / per_word + 1;
    words.min(i64::from(i16::MAX)) as i32
}

/// Слова битмапа вокруг текущего тика: [w − n, w + n] в пределах int16; n — больше из dex.tick_words и слов,
/// которые покрывают полосу цены dex.depth_band_pct при tickSpacing пула (решение 73: при tickSpacing 1 одно слово —
/// 256 тиков ≈ 2,6% цены, и глубина в полосе без них читалась бы не целиком). Полоса не задана — только dex.tick_words
/// (глубина тогда не считается и сама скажет причину). Ошибки: NotConfigured (dex.tick_words), BadResponse
/// (tickSpacing ≤ 0).
pub fn word_range(ctx: &Ctx, tick: i32, spacing: i32) -> GatewayResult<(i16, i16)> {
    if spacing <= 0 {
        return Err(ctx.err(ErrorCode::BadResponse, format!("tickSpacing {spacing} ≤ 0")));
    }
    let n = ctx.int("dex.tick_words")?.min(i16::MAX as u64) as i32;
    let n = match ctx.dec("dex.depth_band_pct") {
        Ok(pct) => n.max(band_words(pct, spacing)),
        Err(_) => n,
    };
    let w = i32::from(TickMap::word_of(tick, spacing));
    let lo = (w - n).max(i32::from(i16::MIN)) as i16;
    let hi = (w + n).min(i32::from(i16::MAX)) as i16;
    Ok((lo, hi))
}

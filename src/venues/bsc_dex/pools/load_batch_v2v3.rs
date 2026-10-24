//! Файл: venues/bsc_dex/pools/load_batch_v2v3.rs
//! Что делает: части пачечного чтения (load_batch.rs): вызовы и разбор ступени 1 (V2 getReserves, V3 slot0 + liquidity;
//! CL — load_batch_cl.rs), слова тиков V3 через TickLens и выполнение ступени (все вызовы всех пулов вместе через
//! node/call.rs try_chunks, ответы — обратно по пулам). Те же функции и разбор по ABI, что у load_v2v3.rs.
//! Кто вызывает: pools/load_batch.rs.
//! От чего зависит: node/call.rs (try_chunks, decode), pools/abi_v2v3.rs, load_batch_cl.rs, load_batch.rs (Want).
//! Настройки: dex.tick_lens [вид пула V3].
//!
//! getReserves / slot0 / liquidity / TickLens.getPopulatedTicksInWord — research/flow/bsc_dex.md, раздел 3
//! (IPancakeV3PoolState.sol#L21-L48, IUniswapV3PoolState.sol#L21-L32, ITickLens.sol#L6-L24; как в load_v2v3.rs).

use std::collections::BTreeMap;

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256};

use crate::core::types::{PoolKind, PoolRef, PoolState};

use super::super::ctx::Ctx;
use super::super::node::call;
use super::abi_v2v3::{IPancakeV3Pool, ITickLens, IUniswapV3Pool, IV2Pair, IV3Pool};
use super::families::Proto;
use super::load_batch::{Answers, Stage, Want};
use super::load_batch_cl as cl;
use super::ticks::TickData;

/// Голова пула: V2 — резервы; V3/CL — состояние и текущий тик.
pub enum PoolStateTick {
    V2(PoolState),
    Ticked(PoolState, i32),
}

/// Вызовы ступени 1 одного пула. Ошибки: причина (нет tickSpacing, нет адреса настройки, ссылка не того вида).
pub fn head(ctx: &Ctx, w: &Want) -> Result<Vec<(Address, Bytes)>, String> {
    let enc = |to: Address, d: Vec<u8>| (to, Bytes::from(d));
    match (w.family.kind, w.reference) {
        (PoolKind::V2, PoolRef::Contract(a)) => {
            Ok(vec![enc(*a, IV2Pair::getReservesCall {}.abi_encode())])
        }
        (PoolKind::V3, PoolRef::Contract(a)) => {
            w.spacing
                .ok_or_else(|| format!("{}: у пула нет tickSpacing", w.family.title))?;
            let slot0 = match w.family.proto {
                Proto::PancakeV3 => IPancakeV3Pool::slot0Call {}.abi_encode(),
                Proto::UniswapV3 => IUniswapV3Pool::slot0Call {}.abi_encode(),
                _ => return Err(format!("{}: не V3", w.family.title)),
            };
            Ok(vec![
                enc(*a, slot0),
                enc(*a, IV3Pool::liquidityCall {}.abi_encode()),
            ])
        }
        (PoolKind::Cl, PoolRef::Singleton { .. }) => {
            w.spacing
                .ok_or_else(|| format!("{}: у пула нет tickSpacing", w.family.title))?;
            cl::head(ctx, w)
        }
        _ => Err(format!("{}: ссылка на пул не того вида", w.family.title)),
    }
}

/// Разбор ступени 1. Ошибки: причина (ответ не по ABI).
pub fn parse_head(w: &Want, b: &[Bytes]) -> Result<PoolStateTick, String> {
    let m = |e: crate::gateway::errors::GatewayError| e.message;
    match w.family.kind {
        PoolKind::V2 => {
            let r = call::decode::<IV2Pair::getReservesCall>(&b[0]).map_err(m)?;
            Ok(PoolStateTick::V2(PoolState::V2 {
                reserve0: U256::from(r.reserve0),
                reserve1: U256::from(r.reserve1),
            }))
        }
        PoolKind::V3 => {
            let (sqrt, tick) = if w.family.proto == Proto::PancakeV3 {
                let s = call::decode::<IPancakeV3Pool::slot0Call>(&b[0]).map_err(m)?;
                (U256::from(s.sqrtPriceX96), s.tick.as_i32())
            } else {
                let s = call::decode::<IUniswapV3Pool::slot0Call>(&b[0]).map_err(m)?;
                (U256::from(s.sqrtPriceX96), s.tick.as_i32())
            };
            let liquidity = call::decode::<IV3Pool::liquidityCall>(&b[1]).map_err(m)?;
            Ok(PoolStateTick::Ticked(
                PoolState::V3 {
                    sqrt_price_x96: sqrt,
                    liquidity,
                    tick,
                },
                tick,
            ))
        }
        _ => cl::parse_head(w, b),
    }
}

/// Вызовы ступени 2 одного пула (слова [lo, hi]). Ошибки: причина (нет адреса dex.tick_lens / dex.state_view).
pub fn words(ctx: &Ctx, w: &Want, (lo, hi): (i16, i16)) -> Result<Vec<(Address, Bytes)>, String> {
    if w.family.kind != PoolKind::V3 {
        return cl::word_calls(ctx, w, (lo, hi));
    }
    let PoolRef::Contract(pool) = w.reference else {
        return Err(format!("{}: ссылка на пул не того вида", w.family.title));
    };
    let lens = ctx
        .dex_addr("dex.tick_lens", w.family.id)
        .map_err(|e| e.message)?;
    Ok((lo..=hi)
        .map(|i| {
            let c = ITickLens::getPopulatedTicksInWordCall {
                pool: *pool,
                tickBitmapIndex: i,
            };
            (lens, Bytes::from(c.abi_encode()))
        })
        .collect())
}

/// Тики V3 из ответов TickLens. Ошибки: причина (ответ не по ABI).
pub fn v3_ticks(ans: &[Bytes]) -> Result<BTreeMap<i32, TickData>, String> {
    let mut ticks = BTreeMap::new();
    for b in ans {
        let word =
            call::decode::<ITickLens::getPopulatedTicksInWordCall>(b).map_err(|e| e.message)?;
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
    Ok(ticks)
}

/// Выполняет ступень: все вызовы всех пулов вместе частями по `size`, ответы — обратно по пулам.
/// Ошибок нет: откат или сбой части — причина у пула.
pub async fn run(ctx: &Ctx, stage: &Stage, tag: &str, size: usize) -> Answers {
    let flat: Vec<(Address, Bytes)> = stage.iter().flatten().cloned().collect();
    let mut pieces = call::try_chunks(ctx, &flat, tag, size).await.into_iter();
    stage
        .iter()
        .map(|calls| {
            let mut got = Vec::with_capacity(calls.len());
            let mut why = None;
            for _ in calls {
                match pieces.next() {
                    Some(Ok(Ok(b))) => got.push(b),
                    Some(Ok(Err(revert))) => {
                        why.get_or_insert(format!("вызов откатился: {revert}"));
                    }
                    Some(Err(node)) => {
                        why.get_or_insert(format!("узел: {node}"));
                    }
                    None => {
                        why.get_or_insert("ответов меньше, чем вызовов".to_string());
                    }
                }
            }
            why.map_or(Ok(got), Err)
        })
        .collect()
}

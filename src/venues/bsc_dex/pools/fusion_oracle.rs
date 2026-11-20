//! Файл: venues/bsc_dex/pools/fusion_oracle.rs
//! Что делает: решение 463 (T463-02) — данные оракула пула THENA Fusion на блоке для комиссии следующего блока
//! (вынесено из fusion_fee.rs — ревью t236, п. 2). Два обращения к узлу пачкой JSON-RPC:
//! 1) dataStorageOperator() пула; timepoints(index − 1, index, index + 1, index + 2, 0) пула; eth_getStorageAt слот 3
//!    пула (liquidity — младшие 128 бит, volumePerLiquidityInBlock — старшие; раскладка PoolState.sol исходника пула
//!    THENA: слоты 0, 1 — totalFeeGrowth0/1Token, 2 — globalState (256 бит), 3 — liquidity |
//!    volumePerLiquidityInBlock); заголовок блока (время);
//! 2) feeConfig() оператора; начало окна на время блока + 0 … + 1 + горизонт — getTimepoints(время блока + 1 +
//!    горизонт, [WINDOW + 1 + горизонт, …, WINDOW], tick, index, liquidity) оператора от имени пула (onlyPool: eth_call
//!    с from = пул, поэтому не Multicall3) ЧАСТЯМИ по CHUNK точек (ревью t236, п. 2): каждая часть — отдельный
//!    eth_call с тем же time, газ одной части не растёт с горизонтом; размер части — константа кода (структура
//!    запроса, не порог торговли; замера газа getTimepoints на точку нет — TASKS T463-02).
//!
//! Откат или ошибка узла на части (пул моложе окна — require 'OLD'; любой другой) — этих значений начала окна нет,
//! причина части сохраняется (Got.gaps) и попадает в ошибку / журнал, если комиссия без них не считается (ветка
//! «старейшая точка моложе окна» обходится без начала окна). Ответ части не по ABI или не той длины — ошибка чтения.
//! Кто вызывает: pools/fusion_fee.rs (next_fee).
//! От чего зависит: ctx.rs, node/call.rs, node/rpc.rs (batch), node/jsonrpc.rs, pools/abi_algebra.rs, load_cl.rs (i24),
//! math/fee_algebra_v1.rs, math/adaptive_fee.rs.
//! Настройки: нет (горизонт передаёт fusion_fee.rs — dex.fusion_fee_horizon_s).
//!
//! Источник: код пула THENA Fusion — Sourcify 56, пул 0x1b9a1120…ef7d, полное совпадение (PoolState.sol,
//! PoolImmutables.sol dataStorageOperator, AlgebraPool.sol timepoints); DataStorageOperator.sol (feeConfig,
//! getTimepoints onlyPool) — cryptoalgebra/AlgebraV1 master, с оператором THENA совпал по ответам записи t237.
//! eth_getStorageAt / eth_getBlockByNumber — execution-apis 16caee27 src/eth/state.yaml, block.yaml.

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256};
use serde_json::{Value, json};

use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::super::ctx::Ctx;
use super::super::math::adaptive_fee::FeeConfig;
use super::super::math::fee_algebra_v1::{Oracle, Timepoint, WINDOW, WindowStart};
use super::super::node::call;
use super::super::node::jsonrpc;
use super::super::node::rpc::{self, SOURCE};
use super::abi_algebra::{IAlgebraPool as P, IDataStorageOperator as Op};
use super::fusion_fee::Head;
use super::load_cl::i24;

/// Точек начала окна в одном eth_call getTimepoints (см. шапку).
pub const CHUNK: usize = 64;

/// Данные оракула, прочитанные на блоке: оракул, время блока, причины отсутствующих частей начала окна.
#[derive(Debug, Clone)]
pub struct Got {
    pub oracle: Oracle,
    pub ts: u32,
    pub gaps: Vec<String>,
}

/// Ошибка BadResponse по пулу. Ошибок нет.
pub fn bad(ctx: &Ctx, pool: Address, what: &str) -> GatewayError {
    ctx.err(
        ErrorCode::BadResponse,
        format!("THENA Fusion {pool}: {what}"),
    )
}

/// eth_call на блоке (от имени `from`, если задан). Ошибок нет.
fn eth_call(from: Option<Address>, to: Address, d: Vec<u8>, tag: &str) -> (&'static str, Value) {
    let data = Bytes::from(d);
    let tx = match from {
        Some(f) => json!({ "from": f, "to": to, "input": data }),
        None => json!({ "to": to, "input": data }),
    };
    ("eth_call", json!([tx, tag]))
}

/// Ответ пачки как значение типа T. Ошибки: откат / ошибка узла на вызове, ответ не того вида.
fn answer<T: serde::de::DeserializeOwned>(
    r: Option<GatewayResult<Box<serde_json::value::RawValue>>>,
) -> GatewayResult<T> {
    let Some(r) = r else {
        let e = super::super::errors::err(ErrorCode::BadResponse, "нет ответа");
        return Err(e);
    };
    jsonrpc::decode(&r?, SOURCE)
}

/// Точка оракула из ответа timepoints(uint256). Ошибки: ответ не по ABI.
fn timepoint(b: &Bytes) -> GatewayResult<Timepoint> {
    let t = call::decode::<P::timepointsCall>(b)?;
    Ok(Timepoint {
        initialized: t.initialized,
        ts: t.blockTimestamp,
        tick_cum: t.tickCumulative.as_i64(),
        vol: t.volatilityCumulative.to::<u128>(),
        avg_tick: t.averageTick.as_i32(),
        vpl: U256::from(t.volumePerLiquidityCumulative),
    })
}

/// Данные оракула пула на блоке (см. шапку). Принимает: окружение, пул, состояние пула, блок-тег, горизонт (с).
/// Возвращает: Got. Ошибки: узел, BadResponse (ответ не по ABI, нет timestamp, часть начала окна не той длины).
pub async fn read(ctx: &Ctx, pool: Address, head: Head, tag: &str, h: u32) -> GatewayResult<Got> {
    let i = head.index;
    let idx = [i.wrapping_sub(1), i, i.wrapping_add(1), i.wrapping_add(2), 0];
    let op_call = P::dataStorageOperatorCall {}.abi_encode();
    let mut a = vec![eth_call(None, pool, op_call, tag)];
    for k in idx {
        let c = P::timepointsCall {
            index: U256::from(k),
        };
        a.push(eth_call(None, pool, c.abi_encode(), tag));
    }
    a.push(("eth_getStorageAt", json!([pool, "0x3", tag])));
    a.push(("eth_getBlockByNumber", json!([tag, false])));
    let mut got = rpc::batch(ctx, &a).await?.into_iter();
    let op = call::decode::<P::dataStorageOperatorCall>(&answer::<Bytes>(got.next())?)?;
    let mut tp = Vec::with_capacity(idx.len());
    for _ in idx {
        tp.push(timepoint(&answer::<Bytes>(got.next())?)?);
    }
    let slot: U256 = answer(got.next())?;
    let block: Value = answer(got.next())?;
    let ts = block["timestamp"]
        .as_str()
        .and_then(|h| u64::from_str_radix(h.trim_start_matches("0x"), 16).ok())
        .ok_or_else(|| bad(ctx, pool, "в заголовке блока нет timestamp"))?;
    // block.timestamp пула — uint32 (PoolState._blockTimestamp: усечение).
    let ts = ts as u32;
    let liquidity = (slot & U256::from(u128::MAX)).to::<u128>();
    let vpl_in_block = (slot >> 128usize).to::<u128>();
    // Начало окна на время блока + k (k = 0 … h + 1): secondsAgo = WINDOW + h + 1 − k от времени блока + 1 + h.
    let n = h + 2;
    let ago: Vec<u32> = (0..n).map(|k| WINDOW + (n - 1 - k)).collect();
    let end = ts.wrapping_add(1).wrapping_add(h);
    let mut b = vec![eth_call(None, op, Op::feeConfigCall {}.abi_encode(), tag)];
    for part in ago.chunks(CHUNK) {
        let gt = Op::getTimepointsCall {
            time: end,
            secondsAgos: part.to_vec(),
            tick: i24(ctx, head.tick)?,
            index: i,
            liquidity,
        };
        b.push(eth_call(Some(pool), op, gt.abi_encode(), tag));
    }
    let mut got = rpc::batch(ctx, &b).await?.into_iter();
    let c = call::decode::<Op::feeConfigCall>(&answer::<Bytes>(got.next())?)?;
    let cfg = FeeConfig {
        alpha1: c.alpha1,
        alpha2: c.alpha2,
        beta1: c.beta1,
        beta2: c.beta2,
        gamma1: c.gamma1,
        gamma2: c.gamma2,
        volume_beta: c.volumeBeta,
        volume_gamma: c.volumeGamma,
        base_fee: c.baseFee,
    };
    let (mut starts, mut gaps) = (Vec::with_capacity(ago.len()), Vec::new());
    for (j, part) in ago.chunks(CHUNK).enumerate() {
        let t0 = ts.wrapping_add((j * CHUNK) as u32);
        let t1 = t0.wrapping_add(part.len() as u32 - 1);
        match answer::<Bytes>(got.next()) {
            Ok(out) => starts.extend(part_starts(&out, t0, part.len())?),
            Err(e) => gaps.push(format!("getTimepoints на время {t0}…{t1}: {}", e.message)),
        }
    }
    let oracle = Oracle {
        cfg,
        index: i,
        prev: tp[0],
        last: tp[1],
        next1: tp[2],
        next2: tp[3],
        zero: tp[4],
        vpl_in_block,
        fee: head.fee,
        starts,
    };
    Ok(Got { oracle, ts, gaps })
}

/// Начало окна из ответа одной части getTimepoints: значение k — время `t0` + k. Принимает: ответ, время первого
/// значения, число значений части. Ошибки: ответ не по ABI или не той длины.
fn part_starts(out: &Bytes, t0: u32, len: usize) -> GatewayResult<Vec<(u32, WindowStart)>> {
    let r = call::decode::<Op::getTimepointsCall>(out)?;
    let n = r.tickCumulatives.len();
    let same = r.volatilityCumulatives.len() == n && r.volumePerAvgLiquiditys.len() == n;
    if n != len || !same {
        return Err(super::super::errors::err(
            ErrorCode::BadResponse,
            format!("getTimepoints: ответ не той длины ({n} значений вместо {len})"),
        ));
    }
    let at = |k: usize| WindowStart {
        tick_cum: r.tickCumulatives[k].as_i64(),
        vol: r.volatilityCumulatives[k].to::<u128>(),
        vpl: r.volumePerAvgLiquiditys[k],
    };
    Ok((0..n).map(|k| (t0.wrapping_add(k as u32), at(k))).collect())
}

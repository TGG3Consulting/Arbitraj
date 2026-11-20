//! Файл: venues/bsc_dex/pools/fusion_fee.rs
//! Что делает: решение 463 (T463-02) — комиссия, которую получит своп пула THENA Fusion (вид thena_fusion) в СЛЕДУЮЩЕМ
//! блоке: чтение данных оракула пула на блоке и расчёт math/fee_algebra_v1.rs. Её и держит PoolState::Algebra.fee
//! вида thena_fusion — котировка и глубина (quote/pool_quote.rs, quote/depth_band.rs) считают с ней. Когда читается:
//! при каждом чтении пула (load_algebra.rs) и после каждого Swap пула на блоке события (events_algebra.rs → Applied::Fee
//! → integral_fee.rs apply → read_fee): Swap меняет точку оракула и volumePerLiquidityInBlock. Опроса нет.
//! Что читается (на блоке события, два обращения к узлу пачкой JSON-RPC):
//! 1) dataStorageOperator() пула; timepoints(index − 1, index, index + 1, index + 2, 0) пула; eth_getStorageAt слот 3
//!    пула (liquidity — младшие 128 бит, volumePerLiquidityInBlock — старшие; раскладка PoolState.sol исходника THENA:
//!    слоты 0, 1 — totalFeeGrowth0/1Token, 2 — globalState (256 бит), 3 — liquidity | volumePerLiquidityInBlock);
//!    заголовок блока (время);
//! 2) feeConfig() оператора; getTimepoints(время + 1, [WINDOW + 1, WINDOW], tick, index, liquidity) оператора от
//!    имени пула (onlyPool: eth_call с from = пул) — начало окна на время блока и + 1 с. Откат getTimepoints (пул
//!    моложе окна — require 'OLD') — начала окна нет; расчёт обходится без него, если оно не нужно (ветка «старейшая
//!    точка моложе окна»), иначе — ошибка.
//! Кто вызывает: pools/load_algebra.rs (next_fee), pools/integral_fee.rs (read_fee — после Swap THENA Fusion).
//! От чего зависит: ctx.rs, node/call.rs, node/rpc.rs (batch), node/jsonrpc.rs, pools/abi_algebra.rs, load_cl.rs (i24),
//! math/fee_algebra_v1.rs, math/adaptive_fee.rs.
//! Настройки: нет (базовое поведение: комиссия обмена — та, что даст контракт).
//!
//! Источник: исходник пула THENA Fusion (Sourcify 56, 0x1b9a1120…ef7d, полное совпадение): PoolState.sol (раскладка
//! хранилища), PoolImmutables.sol (dataStorageOperator), AlgebraPool.sol timepoints / _getNewFee; DataStorageOperator.sol
//! (feeConfig, getTimepoints onlyPool) = cryptoalgebra/AlgebraV1 master. eth_getStorageAt / eth_getBlockByNumber —
//! execution-apis 16caee27 src/eth/state.yaml, block.yaml.

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256};
use serde_json::{Value, json};

use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::super::ctx::Ctx;
use super::super::math::adaptive_fee::FeeConfig;
use super::super::math::fee_algebra_v1::{self, Oracle, Timepoint, WINDOW, WindowStart};
use super::super::node::call;
use super::super::node::jsonrpc;
use super::super::node::rpc::{self, SOURCE};
use super::abi_algebra::{IAlgebraPool as P, IDataStorageOperator as Op};
use super::load_cl::i24;

/// Состояние пула на блоке, нужное комиссии: тик, индекс последней точки оракула, globalState.fee.
#[derive(Debug, Clone, Copy)]
pub struct Head {
    pub tick: i32,
    pub index: u16,
    pub fee: u32,
}

/// Комиссия следующего блока: globalState пула на блоке, затем next_fee. Принимает: окружение, пул, блок-тег.
/// Возвращает: комиссию (пипсы). Ошибки: как у next_fee.
pub async fn read_fee(ctx: &Ctx, pool: Address, tag: &str) -> GatewayResult<u32> {
    let gs = call::view(ctx, pool, &P::globalStateCall {}, tag).await?;
    let head = Head {
        tick: gs.tick.as_i32(),
        index: gs.timepointIndex,
        fee: u32::from(gs.fee),
    };
    next_fee(ctx, pool, head, tag).await
}

/// Комиссия следующего блока по состоянию пула `head` на блоке `tag` (fee_algebra_v1::next_block_fee). Принимает:
/// окружение, пул, состояние, блок-тег. Возвращает: комиссию (пипсы). Ошибки: узел; BadResponse — ответ не по ABI,
/// откат нужного вызова, комиссия не считается (нужного начала окна нет).
pub async fn next_fee(ctx: &Ctx, pool: Address, head: Head, tag: &str) -> GatewayResult<u32> {
    let (o, ts) = read(ctx, pool, head, tag).await?;
    fee_algebra_v1::next_block_fee(&o, ts, head.tick)
        .ok_or_else(|| bad(ctx, pool, "комиссия следующего блока не считается"))
}

/// Ошибка BadResponse по пулу. Ошибок нет.
fn bad(ctx: &Ctx, pool: Address, what: &str) -> GatewayError {
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

/// Данные оракула и время блока (см. шапку). Принимает: окружение, пул, состояние пула, блок-тег. Возвращает:
/// (оракул, время блока). Ошибки: узел, BadResponse.
async fn read(ctx: &Ctx, pool: Address, head: Head, tag: &str) -> GatewayResult<(Oracle, u32)> {
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
    let vpl_in_block = (slot >> 128).to::<u128>();
    let t1 = ts.wrapping_add(1);
    let gt = Op::getTimepointsCall {
        time: t1,
        secondsAgos: vec![WINDOW + 1, WINDOW],
        tick: i24(ctx, head.tick)?,
        index: i,
        liquidity,
    };
    let b = [
        eth_call(None, op, Op::feeConfigCall {}.abi_encode(), tag),
        eth_call(Some(pool), op, gt.abi_encode(), tag),
    ];
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
    // Откат getTimepoints (пул моложе окна) — начала окна нет (см. шапку).
    let starts = match answer::<Bytes>(got.next()) {
        Ok(out) => starts(&out, ts)?,
        Err(_) => Vec::new(),
    };
    let o = Oracle {
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
    Ok((o, ts))
}

/// Начало окна на время блока и + 1 с из ответа getTimepoints([WINDOW + 1, WINDOW]) на время блока + 1. Ошибки:
/// ответ не по ABI или не по 2 значения.
fn starts(out: &Bytes, ts: u32) -> GatewayResult<Vec<(u32, WindowStart)>> {
    let r = call::decode::<Op::getTimepointsCall>(out)?;
    let n = r.tickCumulatives.len();
    let same = r.volatilityCumulatives.len() == n && r.volumePerAvgLiquiditys.len() == n;
    if n != 2 || !same {
        return Err(super::super::errors::err(
            ErrorCode::BadResponse,
            "getTimepoints: ответ не по 2 значения",
        ));
    }
    let at = |k: usize| WindowStart {
        tick_cum: r.tickCumulatives[k].as_i64(),
        vol: r.volatilityCumulatives[k].to::<u128>(),
        vpl: r.volumePerAvgLiquiditys[k],
    };
    Ok(vec![(ts, at(0)), (ts.wrapping_add(1), at(1))])
}

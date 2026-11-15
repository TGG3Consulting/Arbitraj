//! Файл: venues/bsc_dex/pools/load_smardex.rs
//! Что делает: решение 288 — чтение состояния пары SmarDex на блоке одним try_many (Multicall3): getReserves (баланс минус
//! доля протокола), getFictiveReserves, getPriceAverage (средняя цена и время её записи), getPairFees, и время этого блока
//! (Multicall3.getCurrentBlockTimestamp в той же пачке; без dex.multicall3 — eth_getBlockByNumber блока, один раз на блок
//! на все пары — память block_secs). Собирает
//! PoolState::Fictive. Выход обмена зависит от времени блока (средняя цена) и от баланса пары (резерв) — вид
//! перечитывается каждым блоком (per_block.rs).
//! Кто вызывает: pools/load.rs (ветка Proto::Smardex).
//! От чего зависит: ctx.rs, node/call.rs, node/multicall.rs, node/head_age.rs, abi_smardex.rs.
//! Настройки: dex.multicall3 [bsc_dex] (не задан — пачкой JSON-RPC и отдельный запрос времени блока).
//!
//! Источник — SmardexPair.sol (фабрика BSC 0xA8EF…Af33, Sourcify match = SmarDex-Ecosystem/dex-smart-contracts@74d0444
//! deployments/bsc): getReserves L80-L83, getFictiveReserves L86-L89, getPairFees L98-L101, getPriceAverage L104-L112.

use std::sync::Mutex;

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256};

use crate::core::types::{FictiveData, PoolState};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::node::{call, head_age, multicall};
use super::abi_smardex::{IMulticall3Time, ISmardexPair as P};

/// Состояние пары на блоке. Принимает: окружение, адрес пары, блок. Ошибки: узел, вызов откатился, ответ не по ABI.
pub async fn load(ctx: &Ctx, pool: Address, block: u64) -> GatewayResult<PoolState> {
    let bad = |what: String| ctx.err(ErrorCode::BadResponse, format!("SmarDex {pool}: {what}"));
    let mut calls: Vec<(Address, Bytes)> = [
        P::getReservesCall {}.abi_encode(),
        P::getFictiveReservesCall {}.abi_encode(),
        P::getPriceAverageCall {}.abi_encode(),
        P::getPairFeesCall {}.abi_encode(),
    ]
    .into_iter()
    .map(|d| (pool, Bytes::from(d)))
    .collect();
    let mc = multicall::address(ctx);
    if let Some(m) = mc {
        calls.push((
            m,
            Bytes::from(IMulticall3Time::getCurrentBlockTimestampCall {}.abi_encode()),
        ));
    }
    let tag = call::block_tag(block);
    let got = call::try_many(ctx, &calls, &tag).await?;
    if got.len() != calls.len() {
        return Err(bad(format!("ответов {} из {}", got.len(), calls.len())));
    }
    let ok = |i: usize| {
        got[i]
            .as_ref()
            .map_err(|r| bad(format!("вызов {i} откатился: {r}")))
    };
    let r = call::decode::<P::getReservesCall>(ok(0)?)?;
    let f = call::decode::<P::getFictiveReservesCall>(ok(1)?)?;
    let pa = call::decode::<P::getPriceAverageCall>(ok(2)?)?;
    let fees = call::decode::<P::getPairFeesCall>(ok(3)?)?;
    let ts = match mc {
        Some(_) => call::decode::<IMulticall3Time::getCurrentBlockTimestampCall>(ok(4)?)?,
        None => U256::from(block_secs(ctx, block).await?),
    };
    let raw = Raw {
        reserves: (r.reserve0_, r.reserve1_),
        fictive: (f.fictiveReserve0_, f.fictiveReserve1_),
        price: (pa.priceAverage0_, pa.priceAverage1_),
        price_ts: pa.priceAverageLastTimestamp_,
        fees: (U256::from(fees.feesLP_), U256::from(fees.feesPool_)),
        block_ts: ts,
    };
    state(&raw).map_err(|w| bad(w.to_string()))
}

/// Время последнего запрошенного блока (номер, секунды) — без dex.multicall3 время блока запрашивается один раз на блок
/// на все пары SmarDex (перечитывание каждым блоком, per_block.rs), а не на каждую пару.
static BLOCK_TIME: Mutex<Option<(u64, u64)>> = Mutex::new(None);

/// Время блока (секунды Unix): из памяти, если этот блок уже спрашивали, иначе eth_getBlockByNumber (node/head_age.rs).
/// Принимает: окружение, номер блока. Ошибки: узел; BadResponse (узел не знает блок).
async fn block_secs(ctx: &Ctx, block: u64) -> GatewayResult<u64> {
    let hit = *BLOCK_TIME.lock().unwrap_or_else(|p| p.into_inner());
    if let Some((n, t)) = hit
        && n == block
    {
        return Ok(t);
    }
    let t = head_age::fetch(ctx, block, true).await?.secs;
    *BLOCK_TIME.lock().unwrap_or_else(|p| p.into_inner()) = Some((block, t));
    Ok(t)
}

/// Прочитанное с пары (ответы вызовов как есть).
pub struct Raw {
    pub reserves: (U256, U256),
    pub fictive: (U256, U256),
    pub price: (U256, U256),
    pub price_ts: U256,
    pub fees: (U256, U256),
    pub block_ts: U256,
}

/// Сборка состояния из прочитанного (без сети; её же зовут тесты на записанных ответах). Возвращает: Fictive. Ошибка —
/// время не помещается в u64 или время блока раньше записи средней цены (обмен на этом блоке откатился бы — require
/// getUpdatedPriceAverage L84).
pub fn state(r: &Raw) -> Result<PoolState, &'static str> {
    let price_ts =
        u64::try_from(r.price_ts).map_err(|_| "время средней цены не помещается в u64")?;
    let block_ts = u64::try_from(r.block_ts).map_err(|_| "время блока не помещается в u64")?;
    if block_ts < price_ts {
        return Err("время блока раньше записи средней цены");
    }
    Ok(PoolState::Fictive(Box::new(FictiveData {
        reserve0: r.reserves.0,
        reserve1: r.reserves.1,
        fictive0: r.fictive.0,
        fictive1: r.fictive.1,
        price_average0: r.price.0,
        price_average1: r.price.1,
        price_average_ts: price_ts,
        fees_lp: r.fees.0,
        fees_pool: r.fees.1,
        block_ts,
    })))
}

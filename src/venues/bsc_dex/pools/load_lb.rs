//! Файл: venues/bsc_dex/pools/load_lb.rs
//! Что делает: решение 290 — чтение состояния пула Liquidity Book V2.1 на блоке: одним try_many (Multicall3) getActiveId,
//! getStaticFeeParameters, getVariableFeeParameters, getTokenX, getBinStep и время этого блока
//! (Multicall3.getCurrentBlockTimestamp в той же пачке; без dex.multicall3 — eth_getBlockByNumber, один раз на блок —
//! load_smardex::block_secs), затем полоса бинов (load_lb_bins.rs). Собирает PoolState::LbBin. Пачка состояния без бинов
//! (calls / decode) — и для перечитывания всех пулов вида каждым блоком (per_block_lb.rs): изменчивая часть комиссии
//! зависит от времени блока без событий пула.
//! Кто вызывает: pools/load.rs (ветка Proto::LbV21), pools/lb_hooks.rs (load_with — LB 2.2, решение 446),
//! pools/lb_head.rs (calls, decode — голова per_block_lb.rs).
//! От чего зависит: ctx.rs, node/call.rs, node/multicall.rs, abi_lb.rs, abi_smardex.rs (IMulticall3Time),
//! load_smardex.rs (block_secs), load_lb_bins.rs, load_bin.rs (Depth), tokens.rs.
//! Настройки: dex.multicall3, dex.bin_depth_usd [bsc_dex].
//!
//! Источник: lfj-gg/joe-v2@v2.1.0 — src/LBPair.sol L132-L134 getTokenX, L150-L152 getBinStep, L171-L173 getActiveId,
//! L216-L239 getStaticFeeParameters, L248-L260 getVariableFeeParameters.

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes};

use crate::core::types::{LbBinData, PoolState};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::node::{call, multicall};
use super::super::tokens::Tokens;
use super::abi_lb::ILBPair as P;
use super::abi_smardex::IMulticall3Time;
use super::bins::BinMap;
use super::load_bin::Depth;
use super::{load_lb_bins, load_smardex};

/// Вызовов состояния на пул в пачке (getActiveId, getStaticFeeParameters, getVariableFeeParameters).
pub const PER_POOL: usize = 3;

/// Вызовы состояния пула (без бинов). Ошибок нет.
pub fn calls(pool: Address) -> Vec<(Address, Bytes)> {
    [
        P::getActiveIdCall {}.abi_encode(),
        P::getStaticFeeParametersCall {}.abi_encode(),
        P::getVariableFeeParametersCall {}.abi_encode(),
    ]
    .into_iter()
    .map(|d| (pool, Bytes::from(d)))
    .collect()
}

/// Вызов времени блока (Multicall3.getCurrentBlockTimestamp). Ошибок нет.
pub fn time_call(mc: Address) -> (Address, Bytes) {
    (
        mc,
        Bytes::from(IMulticall3Time::getCurrentBlockTimestampCall {}.abi_encode()),
    )
}

/// Разбор ответа времени блока. Ошибка — не по ABI или не помещается в u64.
pub fn decode_time(out: &[u8]) -> Result<u64, String> {
    let t = IMulticall3Time::getCurrentBlockTimestampCall::abi_decode_returns(out)
        .map_err(|e| format!("getCurrentBlockTimestamp не по ABI: {e}"))?;
    u64::try_from(t).map_err(|_| "время блока не помещается в u64".to_string())
}

/// Сборка состояния из ответов calls (без сети; её же зовут тесты на записанных ответах). Принимает: ответы по порядку
/// calls, X — token0 ли, шаг бина, время блока. Возвращает: LbBinData. Ошибка — ответ не по ABI.
pub fn decode(
    a: &[Bytes],
    x_is_token0: bool,
    bin_step: u32,
    block_ts: u64,
) -> Result<LbBinData, String> {
    if a.len() != PER_POOL {
        return Err(format!("ответов {} из {PER_POOL}", a.len()));
    }
    let id =
        P::getActiveIdCall::abi_decode_returns(&a[0]).map_err(|e| format!("getActiveId: {e}"))?;
    let s = P::getStaticFeeParametersCall::abi_decode_returns(&a[1])
        .map_err(|e| format!("getStaticFeeParameters: {e}"))?;
    let v = P::getVariableFeeParametersCall::abi_decode_returns(&a[2])
        .map_err(|e| format!("getVariableFeeParameters: {e}"))?;
    Ok(LbBinData {
        active_id: id.to(),
        bin_step,
        x_is_token0,
        base_factor: u32::from(s.baseFactor),
        filter_period: u32::from(s.filterPeriod),
        decay_period: u32::from(s.decayPeriod),
        reduction_factor: u32::from(s.reductionFactor),
        variable_fee_control: s.variableFeeControl.to(),
        protocol_share: u32::from(s.protocolShare),
        max_volatility_accumulator: s.maxVolatilityAccumulator.to(),
        volatility_accumulator: v.volatilityAccumulator.to(),
        volatility_reference: v.volatilityReference.to(),
        id_reference: v.idReference.to(),
        time_of_last_update: v.timeOfLastUpdate.to(),
        block_ts,
        v20: false,
        hooks_parameters: None,
        swap_blocked: None,
    })
}

/// Состояние и полоса бинов на блоке. Принимает: окружение, токены (decimals USDT), пул, пару (token0, token1), USDT,
/// блок. Ошибки: узел, вызов откатился, ответ не по ABI, X пула не из пары, NotConfigured (dex.bin_depth_usd).
pub async fn load(
    ctx: &Ctx,
    tokens: &Tokens,
    pool: Address,
    pair: (Address, Address),
    usdt: Address,
    block: u64,
) -> GatewayResult<(PoolState, BinMap)> {
    let (d, bins, _) = load_with(ctx, tokens, pool, pair, usdt, block, &[]).await?;
    Ok((PoolState::LbBin(Box::new(d)), bins))
}

/// Как load, но ещё вызовы `extra` в ТОЙ ЖЕ пачке на том же блоке (решение 446, ревью С3: getLBHooksParameters пула
/// LB 2.2 — lb_hooks.rs). Возвращает: состояние, полосу бинов и ответы extra по порядку (откат — Err с текстом: это
/// ответ, решает вызывающий). Ошибки: как у load.
#[allow(clippy::too_many_arguments)]
pub async fn load_with(
    ctx: &Ctx,
    tokens: &Tokens,
    pool: Address,
    pair: (Address, Address),
    usdt: Address,
    block: u64,
    extra: &[(Address, Bytes)],
) -> GatewayResult<(LbBinData, BinMap, Vec<Result<Bytes, String>>)> {
    let bad = |what: String| ctx.err(ErrorCode::BadResponse, format!("LB {pool}: {what}"));
    let mut all = calls(pool);
    all.push((pool, Bytes::from(P::getTokenXCall {}.abi_encode())));
    all.push((pool, Bytes::from(P::getBinStepCall {}.abi_encode())));
    let mc = multicall::address(ctx);
    if let Some(m) = mc {
        all.push(time_call(m));
    }
    let own = all.len();
    all.extend_from_slice(extra);
    let tag = call::block_tag(block);
    let mut got = call::try_many(ctx, &all, &tag).await?;
    if got.len() != all.len() {
        return Err(bad(format!("ответов {} из {}", got.len(), all.len())));
    }
    let rest = got.split_off(own);
    let ok: Vec<Bytes> = got
        .into_iter()
        .enumerate()
        .map(|(i, r)| r.map_err(|w| bad(format!("вызов {i} откатился: {w}"))))
        .collect::<GatewayResult<_>>()?;
    let x = call::decode::<P::getTokenXCall>(&ok[PER_POOL])?;
    let step = u32::from(call::decode::<P::getBinStepCall>(&ok[PER_POOL + 1])?);
    if x != pair.0 && x != pair.1 {
        return Err(bad(format!("токен X {x:#x} не из пары записи")));
    }
    let ts = match mc {
        Some(_) => decode_time(&ok[PER_POOL + 2]).map_err(&bad)?,
        None => load_smardex::block_secs(ctx, block).await?,
    };
    let d = decode(&ok[..PER_POOL], x == pair.0, step, ts).map_err(&bad)?;
    let y = if x == pair.0 { pair.1 } else { pair.0 };
    let depth = if usdt == x || usdt == y {
        Some(Depth {
            usdt_is_y: usdt == y,
            usdt_decimals: tokens.decimals(ctx, usdt).await?,
        })
    } else {
        None
    };
    let bins = load_lb_bins::band(ctx, pool, d.active_id, step, depth.as_ref(), &tag).await?;
    Ok((d, bins, rest))
}

//! Файл: venues/bsc_dex/pools/load_lb20.rs
//! Что делает: решение 445 — чтение состояния пула Liquidity Book v2.0 (LFJ V2, Arbitrum) на блоке: одним try_many
//! (Multicall3) getReservesAndId (активный бин), feeParameters (шаг бина, постоянные и изменчивые параметры комиссии —
//! одна структура), tokenX и время этого блока (Multicall3.getCurrentBlockTimestamp; без dex.multicall3 —
//! eth_getBlockByNumber, load_smardex::block_secs), затем полоса бинов (load_lb20_bins.rs). Собирает PoolState::LbBin с
//! признаком v20 (правила математики v2.0 — math/lb20_*.rs). Пачка состояния без бинов (calls / decode) — и для
//! перечитывания каждым блоком (per_block_lb.rs через lb_head.rs): изменчивая часть комиссии зависит от времени блока
//! без событий пула, а параметры комиссии фабрика меняет без события пары.
//! Кто вызывает: pools/load.rs (ветка Proto::LbV20), pools/lb_head.rs (calls, decode).
//! От чего зависит: ctx.rs, node/call.rs, node/multicall.rs, abi_lb20.rs, load_lb.rs (time_call, decode_time),
//! load_smardex.rs (block_secs), load_lb20_bins.rs, load_bin.rs (Depth), tokens.rs.
//! Настройки: dex.multicall3, dex.bin_depth_usd [площадка DEX].
//!
//! Источник: lfj-gg/joe-v2, тег v2.0.0 (07f10212) — src/LBPair.sol L56 tokenX, L147-L158 getReservesAndId (activeId —
//! младшие 24 бита слота 0 _pairInformation, L1023-L1042), L249-L251 feeParameters (_feeParameters целиком);
//! src/libraries/FeeHelper.sol L29-L44 (поля FeeParameters: time — время последнего обновления).

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes};

use crate::core::types::{LbBinData, PoolState};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::node::{call, multicall};
use super::super::tokens::Tokens;
use super::abi_lb20::ILBPairV20 as P;
use super::bins::BinMap;
use super::load_bin::Depth;
use super::{load_lb, load_lb20_bins, load_smardex};

/// Вызовов состояния на пул в пачке (getReservesAndId, feeParameters).
pub const PER_POOL: usize = 2;

/// Вызовы состояния пула (без бинов). Ошибок нет.
pub fn calls(pool: Address) -> Vec<(Address, Bytes)> {
    [
        P::getReservesAndIdCall {}.abi_encode(),
        P::feeParametersCall {}.abi_encode(),
    ]
    .into_iter()
    .map(|d| (pool, Bytes::from(d)))
    .collect()
}

/// Сборка состояния из ответов calls (без сети; её же зовут тесты на записанных ответах). Принимает: ответы по порядку
/// calls, X — token0 ли, время блока. Возвращает: LbBinData (v20 = true; шаг бина — из feeParameters). Ошибка — ответ
/// не по ABI, активный бин больше uint24.
pub fn decode(a: &[Bytes], x_is_token0: bool, block_ts: u64) -> Result<LbBinData, String> {
    if a.len() != PER_POOL {
        return Err(format!("ответов {} из {PER_POOL}", a.len()));
    }
    let r = P::getReservesAndIdCall::abi_decode_returns(&a[0])
        .map_err(|e| format!("getReservesAndId: {e}"))?;
    let f = P::feeParametersCall::abi_decode_returns(&a[1])
        .map_err(|e| format!("feeParameters: {e}"))?;
    let active_id = u32::try_from(r.activeId)
        .ok()
        .filter(|x| *x <= 0xFF_FFFF)
        .ok_or_else(|| format!("активный бин {} вне uint24", r.activeId))?;
    Ok(LbBinData {
        active_id,
        bin_step: u32::from(f.binStep),
        x_is_token0,
        base_factor: u32::from(f.baseFactor),
        filter_period: u32::from(f.filterPeriod),
        decay_period: u32::from(f.decayPeriod),
        reduction_factor: u32::from(f.reductionFactor),
        variable_fee_control: f.variableFeeControl.to(),
        protocol_share: u32::from(f.protocolShare),
        max_volatility_accumulator: f.maxVolatilityAccumulated.to(),
        volatility_accumulator: f.volatilityAccumulated.to(),
        volatility_reference: f.volatilityReference.to(),
        id_reference: f.indexRef.to(),
        time_of_last_update: f.time.to(),
        block_ts,
        v20: true,
        // Решение 446: крючков пула у v2.0 нет, обмен не исключён (поля LB 2.2).
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
    let bad = |what: String| ctx.err(ErrorCode::BadResponse, format!("LB v2.0 {pool}: {what}"));
    let mut all = calls(pool);
    all.push((pool, Bytes::from(P::tokenXCall {}.abi_encode())));
    let mc = multicall::address(ctx);
    if let Some(m) = mc {
        all.push(load_lb::time_call(m));
    }
    let tag = call::block_tag(block);
    let got = call::try_many(ctx, &all, &tag).await?;
    if got.len() != all.len() {
        return Err(bad(format!("ответов {} из {}", got.len(), all.len())));
    }
    let ok: Vec<Bytes> = got
        .into_iter()
        .enumerate()
        .map(|(i, r)| r.map_err(|w| bad(format!("вызов {i} откатился: {w}"))))
        .collect::<GatewayResult<_>>()?;
    let x = call::decode::<P::tokenXCall>(&ok[PER_POOL])?;
    if x != pair.0 && x != pair.1 {
        return Err(bad(format!("токен X {x:#x} не из пары записи")));
    }
    let ts = match mc {
        Some(_) => load_lb::decode_time(&ok[PER_POOL + 1]).map_err(&bad)?,
        None => load_smardex::block_secs(ctx, block).await?,
    };
    let d = decode(&ok[..PER_POOL], x == pair.0, ts).map_err(&bad)?;
    let y = if x == pair.0 { pair.1 } else { pair.0 };
    let depth = if usdt == x || usdt == y {
        Some(Depth {
            usdt_is_y: usdt == y,
            usdt_decimals: tokens.decimals(ctx, usdt).await?,
        })
    } else {
        None
    };
    let (active, step) = (d.active_id, d.bin_step);
    let bins = load_lb20_bins::band(ctx, pool, active, step, depth.as_ref(), &tag).await?;
    Ok((PoolState::LbBin(Box::new(d)), bins))
}

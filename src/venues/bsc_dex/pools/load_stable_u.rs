//! Файл: venues/bsc_dex/pools/load_stable_u.rs
//! Что делает: чтение состояния пула PancakeSwap StableSwap или Lista StableSwap на блоке (решение 122 F, 3) одним
//! try_many (Multicall3): coins(k)/balances(k) до первого отката coins, fee(); PancakeSwap — A() (сырой, с рампой на
//! блоке) и is_killed(); Lista — initial_A/future_A/initial_A_time/future_A_time (A в точности A_PRECISION считается
//! по времени блока как get_A — A() Lista делит на A_PRECISION и теряет точность), admin_fee, skipPriceDiff, пороги,
//! цены оракула (fetchOraclePrice), paused() и время блока (Multicall3.getCurrentBlockTimestamp). Множители точности —
//! правило `RATES = 1e18·10^(18 − decimals)` = 10^(36 − decimals) (инициализация пула). Собирает PoolState::StableSwap.
//! Нативный BNB (решение «122 X (нативный BNB в стейблсвапах)», pools/stable_native.rs): монета пула 0xEeee… в паре —
//! dex.native_wrapped (WBNB), decimals метки = 18 по initialize пула (decimals() у неё не вызывается — кода нет),
//! индекс метки — StableData.native (шаг исполнителя STABLE_U256_BNB).
//! Кто вызывает: pools/load.rs (ветки Proto::PancakeStable, Proto::ListaStable); части (answers, head, indices, rates,
//! finish) — pools/load_site.rs (решение 295: одно чтение пула PancakeSwap StableSwap на все его пары).
//! От чего зависит: ctx.rs, node/call.rs, node/multicall.rs, tokens.rs, abi_curve.rs, abi_stable_u.rs,
//! stable_native.rs, math/stableswap_v1.rs.
//! Настройки: dex.multicall3 [bsc_dex] (у Lista обязателен — время блока); dex.native_wrapped [bsc_dex] (пул с BNB).
//!
//! Источники: PancakeStableSwapTwoPool.sol (cb079908) `initialize` L113-L151 (RATES), `get_A` L152-L170, `A()` L171;
//! Lista StableSwapPool.sol (moolah eb7d73a) `initialize` L125-L143 (RATES, A·A_PRECISION), `get_A` L161-L178.

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256};

use crate::core::types::{PoolState, StableData, StableGuard};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::super::ctx::Ctx;
use super::super::math::stableswap_v1::rate_for_decimals;
use super::super::node::{call, multicall};
use super::super::tokens::Tokens;
use super::abi_curve::{IStableSwapV1, MAX_COINS};
use super::abi_stable_u::{IListaStable as L, IMulticall3Time, IPancakeStable, LISTA_A_PRECISION};
use super::stable_native::{self, NATIVE, NATIVE_DECIMALS};

/// Состояние пула на блоке. Принимает: окружение, токены, пул, пару, true — Lista (иначе PancakeSwap), блок-тег.
/// Ошибки: узел, ответ не по ABI, монеты пары нет в пуле, decimals > 36, не задан dex.multicall3 (Lista); BadResponse —
/// у пула нативная монета 0xEeee…, а dex.native_wrapped не задана или обёртка тоже среди монет пула.
/// Порядок шагов (решение 295 разделил тело на части, порядок прежний): вызовы → ответы → монеты и балансы → fee() →
/// индексы пары → decimals монет → сборка.
pub async fn load(
    ctx: &Ctx,
    tokens: &Tokens,
    pool: Address,
    pair: (Address, Address),
    lista: bool,
    tag: &str,
) -> GatewayResult<PoolState> {
    let bad = |what: String| ctx.err(ErrorCode::BadResponse, format!("StableSwap {pool}: {what}"));
    let (got, tail) = answers(ctx, pool, lista, tag).await?;
    let (coins, balances) = head(&got, &bad)?;
    // fee() разбирается до индексов пары — как до разделения (ошибка отката fee() — первой).
    let tail_ok = got[tail]
        .as_ref()
        .map_err(|r| bad(format!("вызов {tail} откатился: {r}")))?;
    call::decode::<IStableSwapV1::feeCall>(tail_ok)?;
    let (i0, i1) = indices(ctx, &coins, pair, &bad)?;
    let rates = rates(ctx, tokens, &coins, &bad).await?;
    finish(&got, tail, lista, &coins, balances, rates, (i0, i1), &bad)
}

/// Что делает: вызовы чтения и ответы узла одним try_many. Принимает: окружение, пул, Lista ли, блок-тег.
/// Возвращает: (ответы, индекс fee() в них). Ошибки: не задан dex.multicall3 (Lista), узел, ответов не столько, сколько
/// вызовов.
pub async fn answers(
    ctx: &Ctx,
    pool: Address,
    lista: bool,
    tag: &str,
) -> GatewayResult<(Vec<Result<Bytes, String>>, usize)> {
    let mut calls: Vec<(Address, Bytes)> = Vec::new();
    for k in 0..MAX_COINS {
        let i = U256::from(k);
        calls.push((pool, IStableSwapV1::coinsCall { i }.abi_encode().into()));
        calls.push((pool, IStableSwapV1::balancesCall { i }.abi_encode().into()));
    }
    let tail = calls.len();
    calls.push((pool, IStableSwapV1::feeCall {}.abi_encode().into()));
    if lista {
        let mc = multicall::address(ctx).ok_or_else(|| {
            ctx.not_set(
                "dex.multicall3",
                "Lista StableSwap: время блока для рампы A — нужен dex.multicall3",
            )
        })?;
        for d in [
            L::initial_ACall {}.abi_encode(),
            L::future_ACall {}.abi_encode(),
            L::initial_A_timeCall {}.abi_encode(),
            L::future_A_timeCall {}.abi_encode(),
            L::admin_feeCall {}.abi_encode(),
            L::skipPriceDiffCall {}.abi_encode(),
            L::price0DiffThresholdCall {}.abi_encode(),
            L::price1DiffThresholdCall {}.abi_encode(),
            L::fetchOraclePriceCall {}.abi_encode(),
            L::pausedCall {}.abi_encode(),
        ] {
            calls.push((pool, d.into()));
        }
        calls.push((
            mc,
            IMulticall3Time::getCurrentBlockTimestampCall {}
                .abi_encode()
                .into(),
        ));
    } else {
        calls.push((pool, IStableSwapV1::ACall {}.abi_encode().into()));
        calls.push((pool, IPancakeStable::is_killedCall {}.abi_encode().into()));
    }
    let got = call::try_many(ctx, &calls, tag).await?;
    if got.len() != calls.len() {
        let what = format!("StableSwap {pool}: ответов {} из {}", got.len(), calls.len());
        return Err(ctx.err(ErrorCode::BadResponse, what));
    }
    Ok((got, tail))
}

/// Что делает: монеты и балансы пула по ответам coins(k) / balances(k) (до первого отката coins). Без сети.
/// Ошибки: ответ не по ABI, откат balances(k).
pub fn head(
    got: &[Result<Bytes, String>],
    bad: &dyn Fn(String) -> GatewayError,
) -> GatewayResult<(Vec<Address>, Vec<U256>)> {
    let mut coins = Vec::new();
    let mut balances = Vec::new();
    for k in 0..MAX_COINS {
        let Ok(c) = &got[2 * k] else { break };
        coins.push(call::decode::<IStableSwapV1::coinsCall>(c)?);
        let b = got[2 * k + 1]
            .as_ref()
            .map_err(|r| bad(format!("balances({k}) откатился: {r}")))?;
        balances.push(call::decode::<IStableSwapV1::balancesCall>(b)?);
    }
    Ok((coins, balances))
}

/// Что делает: индексы пары среди монет пула — метка нативной монеты = обёртка (stable_native.rs). Без сети.
/// Ошибки: метка без dex.native_wrapped или обёртка тоже в пуле; монеты пары нет среди coins.
pub fn indices(
    ctx: &Ctx,
    coins: &[Address],
    pair: (Address, Address),
    bad: &dyn Fn(String) -> GatewayError,
) -> GatewayResult<(u8, u8)> {
    let paired = stable_native::pair_coins(coins, stable_native::wrapped(ctx)).map_err(bad)?;
    let index = |t: Address| {
        paired
            .iter()
            .position(|c| *c == t)
            .map(|p| p as u8)
            .ok_or_else(|| bad(format!("монеты {t} нет среди coins пула")))
    };
    Ok((index(pair.0)?, index(pair.1)?))
}

/// Что делает: множители точности монет (10^(36 − decimals); у метки BNB — 18 знаков). Принимает: окружение, токены,
/// монеты. Ошибки: узел (decimals), decimals > 36.
pub async fn rates(
    ctx: &Ctx,
    tokens: &Tokens,
    coins: &[Address],
    bad: &(dyn Fn(String) -> GatewayError + Sync),
) -> GatewayResult<Vec<U256>> {
    let mut rates = Vec::with_capacity(coins.len());
    for c in coins {
        let d = if *c == NATIVE {
            NATIVE_DECIMALS
        } else {
            tokens.decimals(ctx, *c).await?
        };
        rates.push(rate_for_decimals(d).ok_or_else(|| bad(format!("decimals {d} > 36")))?);
    }
    Ok(rates)
}

/// Что делает: сборка состояния из ответов (без сети): fee, A (у Lista — рампа по времени блока, проверка цены,
/// пауза), остановка пула. Принимает: ответы, индекс fee(), Lista ли, монеты, балансы, множители, индексы пары,
/// конструктор ошибки. Ошибки: откат обязательного вызова, ответ не по ABI, рампа A не считается.
#[allow(clippy::too_many_arguments)]
pub fn finish(
    got: &[Result<Bytes, String>],
    tail: usize,
    lista: bool,
    coins: &[Address],
    balances: Vec<U256>,
    rates: Vec<U256>,
    (i0, i1): (u8, u8),
    bad: &dyn Fn(String) -> GatewayError,
) -> GatewayResult<PoolState> {
    let ok = |i: usize| {
        got[i]
            .as_ref()
            .map_err(|r| bad(format!("вызов {i} откатился: {r}")))
    };
    let fee = call::decode::<IStableSwapV1::feeCall>(ok(tail)?)?;
    let base = tail + 1;
    let (amp, amp_precision, guard, halted) = if lista {
        let w = |i: usize| -> GatewayResult<U256> {
            Ok(U256::from_be_slice(ok(base + i)?.get(..32).unwrap_or(&[])))
        };
        let (a0, a1, t0, t1) = (w(0)?, w(1)?, w(2)?, w(3)?);
        let now = call::decode::<IMulticall3Time::getCurrentBlockTimestampCall>(ok(base + 10)?)?;
        let amp = ramp(a0, a1, t0, t1, now).ok_or_else(|| bad("рампа A не считается".into()))?;
        let skip = call::decode::<L::skipPriceDiffCall>(ok(base + 5)?)?;
        let paused = call::decode::<L::pausedCall>(ok(base + 9)?)?;
        // Цены оракула: откат fetchOraclePrice (оракул не отвечает) — обмен с проверкой откатится тоже → пара стоит.
        let guard = match (skip, got[base + 8].as_ref()) {
            (true, _) => Ok(None),
            (false, Ok(b)) => Ok(Some(StableGuard {
                admin_fee: w(4)?,
                oracle: call::decode::<L::fetchOraclePriceCall>(b)?.to_vec(),
                threshold: vec![w(6)?, w(7)?],
            })),
            (false, Err(_)) => Err(()),
        };
        let halted = paused || guard.is_err() || coins.len() != 2;
        (
            amp,
            U256::from(LISTA_A_PRECISION),
            guard.unwrap_or(None),
            halted,
        )
    } else {
        let amp = call::decode::<IStableSwapV1::ACall>(ok(base)?)?;
        let killed = call::decode::<IPancakeStable::is_killedCall>(ok(base + 1)?)?;
        (amp, U256::from(1u8), None, killed)
    };
    Ok(PoolState::StableSwap(Box::new(StableData {
        balances,
        rates,
        amp,
        fee,
        i0,
        i1,
        amp_precision,
        // Обе — 255 раундов без отката (PancakeStableSwapTwoPool.sol L201, L376; StableSwapPool.sol L210, L388).
        max_rounds: 255,
        strict: false,
        guard,
        halted,
        ng: None,
        native: stable_native::native_index(coins),
        input_fee: false,
        pair_d: false,
    })))
}

/// `get_A` (рампа A по времени блока). Принимает: initial_A, future_A, initial_A_time, future_A_time, время блока.
/// Возвращает: A в точности пула; None — арифметика откатилась бы. Ошибок нет.
pub fn ramp(a0: U256, a1: U256, t0: U256, t1: U256, now: U256) -> Option<U256> {
    if now >= t1 {
        return Some(a1);
    }
    let span = t1.checked_sub(t0)?;
    if span.is_zero() {
        return None;
    }
    let el = now.checked_sub(t0)?;
    if a1 > a0 {
        a0.checked_add((a1 - a0).checked_mul(el)? / span)
    } else {
        a0.checked_sub((a0 - a1).checked_mul(el)? / span)
    }
}

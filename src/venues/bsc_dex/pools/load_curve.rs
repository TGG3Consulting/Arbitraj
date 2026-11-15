//! Файл: venues/bsc_dex/pools/load_curve.rs
//! Что делает: чтение состояния пула StableSwap (Ellipsis StableSwap, шаблон Curve 2020; решение 122 F) на блоке —
//! одним try_many (Multicall3, откат вызова — ответ): coins(k) и balances(k) для k = 0..MAX_COINS (первый откат
//! coins(k) — конец списка монет: у Vyper-массива вне границ геттер откатывается), A(), fee(). Множители точности
//! монет — правило Curve `10^(36 − decimals)` по decimals монет (tokens.rs). Индексы пары token0/token1 — по
//! адресам монет пула. Собирает PoolState::StableSwap. Карт тиков/бинов нет.
//! Кто вызывает: pools/load.rs (ветка Proto::CurveStable); calls / parse / build без сети — ещё
//! tests/bsc_dex_stable_list_live.rs (ревью 292). Пара — (token0, token1), token0 < token1 (TASKS 292 п. 6).
//! От чего зависит: ctx.rs, node/call.rs (try_many), tokens.rs, abi_curve.rs, math/stableswap_v1.rs.
//! Настройки: нет (dex.multicall3 — через node/call.rs).
//!
//! Геттеры — ellipsis-finance/ellipsis 6bdf778, contracts/StableSwap.vy L95-L97, L176-L177 (abi_curve.rs).

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256};

use crate::core::types::{PoolState, StableData};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::super::ctx::Ctx;
use super::super::math::stableswap_v1::rate_for_decimals;
use super::super::node::call;
use super::super::tokens::Tokens;
use super::abi_curve::{IStableSwapV1, MAX_COINS};

/// Вызовы чтения: coins(k), balances(k) через один для k < MAX_COINS, затем A(), fee(). Возвращает: вызовы и индекс
/// A(). Ошибок нет.
pub fn calls(pool: Address) -> (Vec<(Address, Bytes)>, usize) {
    let mut calls: Vec<(Address, Bytes)> = Vec::new();
    for k in 0..MAX_COINS {
        let i = U256::from(k);
        calls.push((pool, IStableSwapV1::coinsCall { i }.abi_encode().into()));
        calls.push((pool, IStableSwapV1::balancesCall { i }.abi_encode().into()));
    }
    let tail = calls.len();
    calls.push((pool, IStableSwapV1::ACall {}.abi_encode().into()));
    calls.push((pool, IStableSwapV1::feeCall {}.abi_encode().into()));
    (calls, tail)
}

/// Состояние пула на блоке. Принимает: окружение, токены (decimals), адрес пула, пару (token0, token1), блок-тег.
/// Ошибки: узел, ответ не по ABI, монеты пары нет в пуле, decimals монеты > 36, A/fee откатились.
pub async fn load(
    ctx: &Ctx,
    tokens: &Tokens,
    pool: Address,
    pair: (Address, Address),
    tag: &str,
) -> GatewayResult<PoolState> {
    let (calls, tail) = calls(pool);
    let got = call::try_many(ctx, &calls, tag).await?;
    let bad = |what: String| ctx.err(ErrorCode::BadResponse, format!("StableSwap {pool}: {what}"));
    if got.len() != calls.len() {
        return Err(bad(format!("ответов {} из {}", got.len(), calls.len())));
    }
    let (coins, balances, amp, fee, i0, i1) = parse(&got[..tail], &got[tail..], pair, &bad)?;
    let mut decimals = Vec::with_capacity(coins.len());
    for c in &coins {
        decimals.push(tokens.decimals(ctx, *c).await?);
    }
    build(balances, amp, fee, (i0, i1), &decimals, &bad)
}

/// Решение 295: ответы пула и decimals его монет, прочитанные ОДИН раз для всех пар пула (pools/load_site.rs).
pub struct Raw {
    pub pool: Address,
    /// Ответы calls по порядку.
    pub got: Vec<Result<Bytes, String>>,
    /// Индекс A() в ответах (calls).
    pub tail: usize,
    /// decimals монет пула по порядку coins.
    pub decimals: Vec<u8>,
}

/// Что делает: сетевое чтение пула без пары (решение 295): те же вызовы, что load, и decimals всех монет пула.
/// Принимает: окружение, токены, пул, блок-тег. Возвращает: Raw. Ошибки: узел, ответов не столько, сколько вызовов,
/// ответ coins() не по ABI.
pub async fn read(ctx: &Ctx, tokens: &Tokens, pool: Address, tag: &str) -> GatewayResult<Raw> {
    let (calls, tail) = calls(pool);
    let got = call::try_many(ctx, &calls, tag).await?;
    if got.len() != calls.len() {
        let what = format!("StableSwap {pool}: ответов {} из {}", got.len(), calls.len());
        return Err(ctx.err(ErrorCode::BadResponse, what));
    }
    let mut decimals = Vec::new();
    for k in 0..MAX_COINS {
        let Some(Ok(c)) = got.get(2 * k) else { break };
        let coin = call::decode::<IStableSwapV1::coinsCall>(c)?;
        decimals.push(tokens.decimals(ctx, coin).await?);
    }
    Ok(Raw { pool, got, tail, decimals })
}

/// Что делает: состояние пары из прочитанного (решение 295, без сети) — тот же разбор и сборка, что у load.
/// Принимает: окружение (текст ошибки), Raw, пару (token0, token1). Ошибки: как у parse / build.
pub fn state(ctx: &Ctx, raw: &Raw, pair: (Address, Address)) -> GatewayResult<PoolState> {
    let pool = raw.pool;
    state_with(raw, pair, &|what: String| ctx.err(ErrorCode::BadResponse, format!("StableSwap {pool}: {what}")))
}

/// Что делает: state с заданным конструктором ошибки (тесты на записанных ответах — без окружения).
/// Принимает: Raw, пару, конструктор ошибки. Ошибки: как у parse / build.
pub fn state_with(
    raw: &Raw,
    pair: (Address, Address),
    bad: &dyn Fn(String) -> GatewayError,
) -> GatewayResult<PoolState> {
    let (_, balances, amp, fee, i0, i1) = parse(&raw.got[..raw.tail], &raw.got[raw.tail..], pair, bad)?;
    build(balances, amp, fee, (i0, i1), &raw.decimals, bad)
}

/// Разобранные ответы: монеты, балансы, A, fee, индексы пары.
pub type Parsed = (Vec<Address>, Vec<U256>, U256, U256, u8, u8);

/// Разбор ответов без сети (тест ловит изменения загрузки — ревью 292). Принимает: ответы coins(k) / balances(k)
/// через один (до первого отката coins), ответы A() и fee(), пару (token0, token1), конструктор ошибки.
/// Ошибки: ответ не по ABI, откат balances / A / fee, монеты пары нет в пуле.
pub fn parse(
    head: &[Result<Bytes, String>],
    tail: &[Result<Bytes, String>],
    pair: (Address, Address),
    bad: &dyn Fn(String) -> GatewayError,
) -> GatewayResult<Parsed> {
    let mut coins = Vec::new();
    let mut balances = Vec::new();
    for k in 0..MAX_COINS {
        let Some(Ok(c)) = head.get(2 * k) else { break };
        let coin = call::decode::<IStableSwapV1::coinsCall>(c)?;
        let b = match head.get(2 * k + 1) {
            Some(Ok(b)) => b,
            Some(Err(r)) => return Err(bad(format!("balances({k}) откатился: {r}"))),
            None => return Err(bad(format!("нет ответа balances({k})"))),
        };
        coins.push(coin);
        balances.push(call::decode::<IStableSwapV1::balancesCall>(b)?);
    }
    let ok = |i: usize, name: &str| match tail.get(i) {
        Some(Ok(b)) => Ok(b),
        Some(Err(r)) => Err(bad(format!("{name} откатился: {r}"))),
        None => Err(bad(format!("нет ответа {name}"))),
    };
    let amp = call::decode::<IStableSwapV1::ACall>(ok(0, "A()")?)?;
    let fee = call::decode::<IStableSwapV1::feeCall>(ok(1, "fee()")?)?;
    let index = |t: Address| {
        coins
            .iter()
            .position(|c| *c == t)
            .map(|p| p as u8)
            .ok_or_else(|| bad(format!("монеты {t} нет среди coins пула")))
    };
    let (i0, i1) = (index(pair.0)?, index(pair.1)?);
    Ok((coins, balances, amp, fee, i0, i1))
}

/// Состояние из разбора и decimals монет пула (по порядку coins). Ошибки: decimals > 36.
pub fn build(
    balances: Vec<U256>,
    amp: U256,
    fee: U256,
    (i0, i1): (u8, u8),
    decimals: &[u8],
    bad: &dyn Fn(String) -> GatewayError,
) -> GatewayResult<PoolState> {
    let mut rates = Vec::with_capacity(decimals.len());
    for &d in decimals {
        rates.push(rate_for_decimals(d).ok_or_else(|| bad(format!("decimals {d} > 36")))?);
    }
    Ok(PoolState::StableSwap(Box::new(StableData {
        balances,
        rates,
        amp,
        fee,
        i0,
        i1,
        // Шаблон Curve 2020 (stableswap_v1::curve2020): сырой A, 255 раундов, без отката; проверки цены и паузы нет.
        amp_precision: U256::from(1u8),
        max_rounds: 255,
        strict: false,
        guard: None,
        halted: false,
        ng: None,
        native: None,
        input_fee: false,
    })))
}

/// Монеты пула по ответам coins(k) (без сети): до первого отката. Нужна проверке пула (verify_curve.rs).
/// Принимает: ответы coins(0..MAX_COINS). Возвращает: адреса монет или текст «не по ABI». Ошибок нет.
pub fn coins_of(answers: &[Result<Bytes, String>]) -> Result<Vec<Address>, String> {
    let mut out = Vec::new();
    for a in answers {
        let Ok(b) = a else { break };
        out.push(
            IStableSwapV1::coinsCall::abi_decode_returns(b)
                .map_err(|e| format!("ответ coins() не по ABI: {e}"))?,
        );
    }
    Ok(out)
}

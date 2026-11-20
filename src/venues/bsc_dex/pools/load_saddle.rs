//! Файл: venues/bsc_dex/pools/load_saddle.rs
//! Что делает: чтение состояния пула Saddle StableSwap (Nerve; решение 122 F, 3) на блоке одним try_many (Multicall3):
//! getToken(k) и getTokenBalance(k) до первого отката getToken (за последним индексом — «Out of range»),
//! getAPrecise() (A·A_PRECISION с рампой на блоке), swapStorage() (swapFee над 10^10), paused(). Множители точности —
//! 10^(18 − decimals) монеты (конструктор Swap.sol), как rate = multiplier·1e18 = 10^(36 − decimals) в общей
//! математике. Собирает PoolState::StableSwap (A_PRECISION = 100, 256 раундов, откат при несходимости).
//! Кто вызывает: pools/load.rs (ветка Proto::SaddleStable); calls / parse / build без сети — ещё
//! tests/bsc_dex_stable_list_live.rs (ревью 292). Пара — (token0, token1), token0 < token1 (TASKS 292 п. 6).
//! От чего зависит: ctx.rs, node/call.rs, tokens.rs, abi_saddle.rs, math/stableswap_v1.rs.
//! Настройки: нет (dex.multicall3 — через node/call.rs).
//!
//! Источник: nerve-finance/contracts bda7578 — Swap.sol L133-L155 (precisionMultipliers = 10^(18 − decimals)),
//! SwapUtils.sol L150 (MAX_LOOP_LIMIT = 256), L153 (A_PRECISION = 100), getD L393-L430 / getY L531-L582 (откат).
//! Оговорка: множитель пула задаётся параметром конструктора decimals, а не decimals() токена (публичного геттера
//! множителей нет) — расхождение поймает сверка с квотером calculateSwap (решение 73).
//! Решение 451: тот же разбор у пулов Saddle Arbitrum (saddle-contract 2cfdd055: Swap.sol L122-L150 — множители
//! 10^(18 − decimals), SwapUtils.sol — те же getD/getY/_calculateSwap); swapStorage — 7 полей (abi_saddle::swap_fee).

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256};

use crate::core::types::{PoolState, StableData};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::super::ctx::Ctx;
use super::super::math::stableswap_v1::rate_for_decimals;
use super::super::node::call;
use super::super::tokens::Tokens;
use super::abi_saddle::{ISaddleSwap as S, MAX_TOKENS, swap_fee};

/// Монеты пула по ответам getToken(k) (без сети): до первого отката. Нужна проверке пула (verify_saddle.rs).
/// Принимает: ответы getToken(0..MAX_TOKENS). Возвращает: адреса или текст «не по ABI». Ошибок нет.
pub fn tokens_of(answers: &[Result<Bytes, String>]) -> Result<Vec<Address>, String> {
    let mut out = Vec::new();
    for a in answers {
        let Ok(b) = a else { break };
        out.push(
            S::getTokenCall::abi_decode_returns(b)
                .map_err(|e| format!("ответ getToken() не по ABI: {e}"))?,
        );
    }
    Ok(out)
}

/// Вызовы чтения: getToken(k), getTokenBalance(k) через один для k < MAX_TOKENS, затем getAPrecise, swapStorage,
/// paused. Возвращает: вызовы и индекс первого из трёх последних. Ошибок нет.
pub fn calls(pool: Address) -> (Vec<(Address, Bytes)>, usize) {
    let mut calls: Vec<(Address, Bytes)> = Vec::new();
    for k in 0..MAX_TOKENS {
        let index = k as u8;
        calls.push((pool, S::getTokenCall { index }.abi_encode().into()));
        calls.push((pool, S::getTokenBalanceCall { index }.abi_encode().into()));
    }
    let tail = calls.len();
    calls.push((pool, S::getAPreciseCall {}.abi_encode().into()));
    calls.push((pool, S::swapStorageCall {}.abi_encode().into()));
    calls.push((pool, S::pausedCall {}.abi_encode().into()));
    (calls, tail)
}

/// Состояние пула на блоке. Принимает: окружение, токены, пул, пару (token0, token1), блок-тег.
/// Ошибки: узел, ответ не по ABI, монеты пары нет в пуле, decimals > 18.
pub async fn load(
    ctx: &Ctx,
    tokens: &Tokens,
    pool: Address,
    pair: (Address, Address),
    tag: &str,
) -> GatewayResult<PoolState> {
    let bad = |what: String| ctx.err(ErrorCode::BadResponse, format!("Saddle {pool}: {what}"));
    let (calls, tail) = calls(pool);
    let got = call::try_many(ctx, &calls, tag).await?;
    if got.len() != calls.len() {
        return Err(bad(format!("ответов {} из {}", got.len(), calls.len())));
    }
    let p = parse(&got[..tail], &got[tail..], pair, &bad)?;
    let mut decimals = Vec::with_capacity(p.coins.len());
    for c in &p.coins {
        decimals.push(tokens.decimals(ctx, *c).await?);
    }
    build(p, &decimals, &bad)
}

/// Решение 295: ответы пула и decimals его монет, прочитанные ОДИН раз для всех пар пула (pools/load_site.rs).
pub struct Raw {
    pub pool: Address,
    /// Ответы calls по порядку.
    pub got: Vec<Result<Bytes, String>>,
    /// Индекс getAPrecise в ответах (calls).
    pub tail: usize,
    /// decimals монет пула по порядку getToken.
    pub decimals: Vec<u8>,
}

/// Что делает: сетевое чтение пула без пары (решение 295): те же вызовы, что load, и decimals всех монет пула.
/// Принимает: окружение, токены, пул, блок-тег. Возвращает: Raw. Ошибки: узел, ответов не столько, сколько вызовов,
/// ответ getToken() не по ABI.
pub async fn read(ctx: &Ctx, tokens: &Tokens, pool: Address, tag: &str) -> GatewayResult<Raw> {
    let (calls, tail) = calls(pool);
    let got = call::try_many(ctx, &calls, tag).await?;
    if got.len() != calls.len() {
        let what = format!("Saddle {pool}: ответов {} из {}", got.len(), calls.len());
        return Err(ctx.err(ErrorCode::BadResponse, what));
    }
    let mut decimals = Vec::new();
    for k in 0..MAX_TOKENS {
        let Some(Ok(c)) = got.get(2 * k) else { break };
        let coin = call::decode::<S::getTokenCall>(c)?;
        decimals.push(tokens.decimals(ctx, coin).await?);
    }
    Ok(Raw { pool, got, tail, decimals })
}

/// Что делает: состояние пары из прочитанного (решение 295, без сети) — тот же разбор и сборка, что у load.
/// Принимает: окружение (текст ошибки), Raw, пару (token0, token1). Ошибки: как у parse / build.
pub fn state(ctx: &Ctx, raw: &Raw, pair: (Address, Address)) -> GatewayResult<PoolState> {
    let pool = raw.pool;
    state_with(raw, pair, &|what: String| ctx.err(ErrorCode::BadResponse, format!("Saddle {pool}: {what}")))
}

/// Что делает: state с заданным конструктором ошибки (тесты на записанных ответах — без окружения).
/// Принимает: Raw, пару, конструктор ошибки. Ошибки: как у parse / build.
pub fn state_with(
    raw: &Raw,
    pair: (Address, Address),
    bad: &dyn Fn(String) -> GatewayError,
) -> GatewayResult<PoolState> {
    let p = parse(&raw.got[..raw.tail], &raw.got[raw.tail..], pair, bad)?;
    build(p, &raw.decimals, bad)
}

/// Разобранные ответы пула (до decimals монет): монеты, балансы, A·A_PRECISION, swapFee, пауза, индексы пары.
#[derive(Debug, Clone)]
pub struct Parsed {
    pub coins: Vec<Address>,
    pub balances: Vec<U256>,
    pub amp: U256,
    pub fee: U256,
    pub paused: bool,
    pub i0: u8,
    pub i1: u8,
}

/// Разбор ответов без сети (тест ловит изменения загрузки — ревью 292). Принимает: ответы getToken(k) /
/// getTokenBalance(k) через один (до первого отката getToken), ответы getAPrecise, swapStorage, paused, пару
/// (token0, token1), конструктор ошибки. Ошибки: ответ не по ABI, откат обязательного вызова, монеты пары нет в пуле.
pub fn parse(
    head: &[Result<Bytes, String>],
    tail: &[Result<Bytes, String>],
    pair: (Address, Address),
    bad: &dyn Fn(String) -> GatewayError,
) -> GatewayResult<Parsed> {
    let mut coins = Vec::new();
    let mut balances = Vec::new();
    for k in 0..MAX_TOKENS {
        let Some(Ok(c)) = head.get(2 * k) else { break };
        coins.push(call::decode::<S::getTokenCall>(c)?);
        let b = match head.get(2 * k + 1) {
            Some(Ok(b)) => b,
            Some(Err(r)) => return Err(bad(format!("getTokenBalance({k}) откатился: {r}"))),
            None => return Err(bad(format!("нет ответа getTokenBalance({k})"))),
        };
        balances.push(call::decode::<S::getTokenBalanceCall>(b)?);
    }
    let ok = |i: usize| match tail.get(i) {
        Some(Ok(b)) => Ok(b),
        Some(Err(r)) => Err(bad(format!("вызов {} откатился: {r}", head.len() + i))),
        None => Err(bad(format!("нет ответа {}", head.len() + i))),
    };
    let amp = call::decode::<S::getAPreciseCall>(ok(0)?)?;
    // Решение 451: swapStorage Nerve (10 полей) и Saddle saddle-contract (7 полей) — abi_saddle::swap_fee.
    let raw = ok(1)?;
    let fee = swap_fee(raw).ok_or_else(|| {
        bad(format!(
            "swapStorage() не по ABI (не 7 и не 10 слов): длина {} байт, ответ {raw}",
            raw.len()
        ))
    })?;
    let paused = call::decode::<S::pausedCall>(ok(2)?)?;
    let index = |t: Address| {
        coins
            .iter()
            .position(|c| *c == t)
            .map(|p| p as u8)
            .ok_or_else(|| bad(format!("монеты {t} нет среди getToken пула")))
    };
    let (i0, i1) = (index(pair.0)?, index(pair.1)?);
    Ok(Parsed {
        coins,
        balances,
        amp,
        fee,
        paused,
        i0,
        i1,
    })
}

/// Состояние из разбора и decimals монет пула (по порядку coins). Ошибки: decimals > 18.
pub fn build(
    p: Parsed,
    decimals: &[u8],
    bad: &dyn Fn(String) -> GatewayError,
) -> GatewayResult<PoolState> {
    let mut rates = Vec::with_capacity(decimals.len());
    for &d in decimals {
        if d > 18 {
            return Err(bad(format!(
                "decimals {d} > 18 (Swap.sol: «Token decimals exceeds max»)"
            )));
        }
        rates.push(rate_for_decimals(d).ok_or_else(|| bad(format!("decimals {d}")))?);
    }
    let Parsed {
        balances,
        amp,
        fee,
        paused,
        i0,
        i1,
        ..
    } = p;
    Ok(PoolState::StableSwap(Box::new(StableData {
        balances,
        rates,
        amp,
        fee,
        i0,
        i1,
        amp_precision: U256::from(100u8),
        max_rounds: 256,
        strict: true,
        guard: None,
        halted: paused,
        ng: None,
        native: None,
        input_fee: false,
        pair_d: false,
    })))
}

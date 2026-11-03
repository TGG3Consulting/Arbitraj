//! Файл: venues/bsc_dex/pools/load_saddle.rs
//! Что делает: чтение состояния пула Saddle StableSwap (Nerve; решение 122 F, 3) на блоке одним try_many (Multicall3):
//! getToken(k) и getTokenBalance(k) до первого отката getToken (за последним индексом — «Out of range»),
//! getAPrecise() (A·A_PRECISION с рампой на блоке), swapStorage() (swapFee над 10^10), paused(). Множители точности —
//! 10^(18 − decimals) монеты (конструктор Swap.sol), как rate = multiplier·1e18 = 10^(36 − decimals) в общей
//! математике. Собирает PoolState::StableSwap (A_PRECISION = 100, 256 раундов, откат при несходимости).
//! Кто вызывает: pools/load.rs (ветка Proto::SaddleStable).
//! От чего зависит: ctx.rs, node/call.rs, tokens.rs, abi_saddle.rs, math/stableswap_v1.rs.
//! Настройки: нет (dex.multicall3 — через node/call.rs).
//!
//! Источник: nerve-finance/contracts bda7578 — Swap.sol L133-L155 (precisionMultipliers = 10^(18 − decimals)),
//! SwapUtils.sol L150 (MAX_LOOP_LIMIT = 256), L153 (A_PRECISION = 100), getD L393-L430 / getY L531-L582 (откат).
//! Оговорка: множитель пула задаётся параметром конструктора decimals, а не decimals() токена (публичного геттера
//! множителей нет) — расхождение поймает сверка с квотером calculateSwap (решение 73).

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256};

use crate::core::types::{PoolState, StableData};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::math::stableswap_v1::rate_for_decimals;
use super::super::node::call;
use super::super::tokens::Tokens;
use super::abi_saddle::{ISaddleSwap as S, MAX_TOKENS};

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
    let got = call::try_many(ctx, &calls, tag).await?;
    if got.len() != calls.len() {
        return Err(bad(format!("ответов {} из {}", got.len(), calls.len())));
    }
    let mut coins = Vec::new();
    let mut balances = Vec::new();
    for k in 0..MAX_TOKENS {
        let Ok(c) = &got[2 * k] else { break };
        coins.push(call::decode::<S::getTokenCall>(c)?);
        let b = got[2 * k + 1]
            .as_ref()
            .map_err(|r| bad(format!("getTokenBalance({k}) откатился: {r}")))?;
        balances.push(call::decode::<S::getTokenBalanceCall>(b)?);
    }
    let ok = |i: usize| {
        got[i]
            .as_ref()
            .map_err(|r| bad(format!("вызов {i} откатился: {r}")))
    };
    let amp = call::decode::<S::getAPreciseCall>(ok(tail)?)?;
    let fee = call::decode::<S::swapStorageCall>(ok(tail + 1)?)?.swapFee;
    let paused = call::decode::<S::pausedCall>(ok(tail + 2)?)?;
    let index = |t: Address| {
        coins
            .iter()
            .position(|c| *c == t)
            .map(|p| p as u8)
            .ok_or_else(|| bad(format!("монеты {t} нет среди getToken пула")))
    };
    let (i0, i1) = (index(pair.0)?, index(pair.1)?);
    let mut rates = Vec::with_capacity(coins.len());
    for c in &coins {
        let d = tokens.decimals(ctx, *c).await?;
        if d > 18 {
            return Err(bad(format!(
                "decimals {d} > 18 (Swap.sol: «Token decimals exceeds max»)"
            )));
        }
        rates.push(rate_for_decimals(d).ok_or_else(|| bad(format!("decimals {d}")))?);
    }
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
        native: None,
    })))
}

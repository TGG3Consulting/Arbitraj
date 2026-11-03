//! Файл: venues/bsc_dex/pools/load_curve.rs
//! Что делает: чтение состояния пула StableSwap (Ellipsis StableSwap, шаблон Curve 2020; решение 122 F) на блоке —
//! одним try_many (Multicall3, откат вызова — ответ): coins(k) и balances(k) для k = 0..MAX_COINS (первый откат
//! coins(k) — конец списка монет: у Vyper-массива вне границ геттер откатывается), A(), fee(). Множители точности
//! монет — правило Curve `10^(36 − decimals)` по decimals монет (tokens.rs). Индексы пары token0/token1 — по
//! адресам монет пула. Собирает PoolState::StableSwap. Карт тиков/бинов нет.
//! Кто вызывает: pools/load.rs (ветка Proto::CurveStable).
//! От чего зависит: ctx.rs, node/call.rs (try_many), tokens.rs, abi_curve.rs, math/stableswap_v1.rs.
//! Настройки: нет (dex.multicall3 — через node/call.rs).
//!
//! Геттеры — ellipsis-finance/ellipsis 6bdf778, contracts/StableSwap.vy L95-L97, L176-L177 (abi_curve.rs).

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256};

use crate::core::types::{PoolState, StableData};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::math::stableswap_v1::rate_for_decimals;
use super::super::node::call;
use super::super::tokens::Tokens;
use super::abi_curve::{IStableSwapV1, MAX_COINS};

/// Состояние пула на блоке. Принимает: окружение, токены (decimals), адрес пула, пару (token0, token1), блок-тег.
/// Ошибки: узел, ответ не по ABI, монеты пары нет в пуле, decimals монеты > 36, A/fee откатились.
pub async fn load(
    ctx: &Ctx,
    tokens: &Tokens,
    pool: Address,
    pair: (Address, Address),
    tag: &str,
) -> GatewayResult<PoolState> {
    let mut calls: Vec<(Address, Bytes)> = Vec::new();
    for k in 0..MAX_COINS {
        let i = U256::from(k);
        calls.push((pool, IStableSwapV1::coinsCall { i }.abi_encode().into()));
        calls.push((pool, IStableSwapV1::balancesCall { i }.abi_encode().into()));
    }
    calls.push((pool, IStableSwapV1::ACall {}.abi_encode().into()));
    calls.push((pool, IStableSwapV1::feeCall {}.abi_encode().into()));
    let got = call::try_many(ctx, &calls, tag).await?;
    let bad = |what: String| ctx.err(ErrorCode::BadResponse, format!("StableSwap {pool}: {what}"));
    if got.len() != calls.len() {
        return Err(bad(format!("ответов {} из {}", got.len(), calls.len())));
    }
    let mut coins = Vec::new();
    let mut balances = Vec::new();
    for k in 0..MAX_COINS {
        let Ok(c) = &got[2 * k] else { break };
        let coin = call::decode::<IStableSwapV1::coinsCall>(c)?;
        let b = got[2 * k + 1]
            .as_ref()
            .map_err(|r| bad(format!("balances({k}) откатился: {r}")))?;
        coins.push(coin);
        balances.push(call::decode::<IStableSwapV1::balancesCall>(b)?);
    }
    let tail = |i: usize, name: &str| {
        got[i]
            .as_ref()
            .map_err(|r| bad(format!("{name} откатился: {r}")))
    };
    let amp = call::decode::<IStableSwapV1::ACall>(tail(2 * MAX_COINS, "A()")?)?;
    let fee = call::decode::<IStableSwapV1::feeCall>(tail(2 * MAX_COINS + 1, "fee()")?)?;
    let index = |t: Address| {
        coins
            .iter()
            .position(|c| *c == t)
            .map(|p| p as u8)
            .ok_or_else(|| bad(format!("монеты {t} нет среди coins пула")))
    };
    let (i0, i1) = (index(pair.0)?, index(pair.1)?);
    let mut rates = Vec::with_capacity(coins.len());
    for c in &coins {
        let d = tokens.decimals(ctx, *c).await?;
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

//! Файл: venues/bsc_dex/pools/load_nomi.rs
//! Что делает: решение 284 — чтение состояния пары Nomiswap Stable на блоке одним try_many (Multicall3): token0(),
//! getReserves(), swapFee(), getA() (A·100 с рампой по времени блока), token0/1PrecisionMultiplier(). Собирает
//! PoolState::StableSwap шаблона input_fee (math/stableswap_nomi.rs): балансы = резервы пары, rates = множитель·1e18,
//! fee = swapFee·10^5 (над 10^10 — та же дробь, что swapFee / MAX_FEE), A_PRECISION = 100, 256 раундов без отката.
//! Комиссия (setSwapFee без события) и рампа A меняются без события — вид перечитывается каждым блоком (per_block.rs).
//! Кто вызывает: pools/load.rs (ветка Proto::NomiStable).
//! От чего зависит: ctx.rs, node/call.rs, abi_nomi.rs, abi_check.rs (token0).
//! Настройки: dex.multicall3 [bsc_dex] (не задан — пачкой JSON-RPC, call::try_many).
//!
//! Источник — verified-исходник Sourcify full match, chain 56, 0x1e40450f8e21bb68490d7d91ab422888fb3d60f1,
//! contracts/NomiswapStablePair.sol: swapFee L47, MAX_FEE L20, A_PRECISION L21, MAX_LOOP_LIMIT L28, множители L40-L41
//! (10^(18 − decimals), L55, L58), getReserves L276-L280, getA L286-L303, getAmountOut L251-L274 (как берутся резервы,
//! множители, A и комиссия).

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256};

use crate::core::types::{PoolState, StableData};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::math::stableswap_v1::precision;
use super::super::node::call;
use super::abi_check::IPoolImmutables;
use super::abi_nomi::{A_PRECISION, INomiStablePair as P, MAX_FEE, MAX_ROUNDS};

/// Состояние пары на блоке. Принимает: окружение, адрес пары, пару (token0, token1) из проверки, блок-тег. Ошибки:
/// узел, вызов откатился, ответ не по ABI, token0() пары не совпал ни с одной стороной пары.
pub async fn load(
    ctx: &Ctx,
    pool: Address,
    pair: (Address, Address),
    tag: &str,
) -> GatewayResult<PoolState> {
    let bad = |what: String| {
        ctx.err(
            ErrorCode::BadResponse,
            format!("Nomiswap Stable {pool}: {what}"),
        )
    };
    let calls: Vec<(Address, Bytes)> = [
        IPoolImmutables::token0Call {}.abi_encode(),
        P::getReservesCall {}.abi_encode(),
        P::swapFeeCall {}.abi_encode(),
        P::getACall {}.abi_encode(),
        P::token0PrecisionMultiplierCall {}.abi_encode(),
        P::token1PrecisionMultiplierCall {}.abi_encode(),
    ]
    .into_iter()
    .map(|d| (pool, Bytes::from(d)))
    .collect();
    let got = call::try_many(ctx, &calls, tag).await?;
    if got.len() != calls.len() {
        return Err(bad(format!("ответов {} из {}", got.len(), calls.len())));
    }
    let ok = |i: usize| {
        got[i]
            .as_ref()
            .map_err(|r| bad(format!("вызов {i} откатился: {r}")))
    };
    let token0 = call::decode::<IPoolImmutables::token0Call>(ok(0)?)?;
    let r = call::decode::<P::getReservesCall>(ok(1)?)?;
    let fee = call::decode::<P::swapFeeCall>(ok(2)?)?;
    let amp = call::decode::<P::getACall>(ok(3)?)?;
    let m0 = call::decode::<P::token0PrecisionMultiplierCall>(ok(4)?)?;
    let m1 = call::decode::<P::token1PrecisionMultiplierCall>(ok(5)?)?;
    let reserves = (U256::from(r.reserve0), U256::from(r.reserve1));
    state(token0, pair, reserves, fee, amp, (m0, m1)).map_err(|w| bad(w.to_string()))
}

/// Сборка состояния из прочитанного (без сети; её же зовут тесты на записанных ответах). Принимает: token0() пары,
/// пару (token0, token1) записи, резервы, swapFee, getA(), множители. Возвращает: StableSwap шаблона input_fee.
/// Ошибки: token0() пары — не сторона пары записи; swapFee > MAX_FEE (пара так не позволяет — L72).
pub fn state(
    token0: Address,
    pair: (Address, Address),
    reserves: (U256, U256),
    swap_fee: u32,
    amp: U256,
    mult: (u128, u128),
) -> Result<PoolState, &'static str> {
    // Порядок монет пула — token0 / token1 пары; индексы стороны записи — по token0().
    let (i0, i1) = if pair.0 == token0 {
        (0u8, 1u8)
    } else if pair.1 == token0 {
        (1u8, 0u8)
    } else {
        return Err("token0() пары — не сторона пары записи");
    };
    if u64::from(swap_fee) > MAX_FEE {
        return Err("swapFee больше MAX_FEE");
    }
    let p = precision();
    // fee над 10^10 = swapFee · (10^10 / MAX_FEE): дробь та же — floor входа после комиссии совпадает (stableswap_nomi.rs).
    let scale = super::super::math::stableswap_v1::fee_denominator() / U256::from(MAX_FEE);
    Ok(PoolState::StableSwap(Box::new(StableData {
        balances: vec![reserves.0, reserves.1],
        rates: vec![U256::from(mult.0) * p, U256::from(mult.1) * p],
        amp,
        fee: U256::from(swap_fee) * scale,
        amp_precision: U256::from(A_PRECISION),
        max_rounds: MAX_ROUNDS,
        // Несходимость за MAX_LOOP_LIMIT не откатывает — цикл кончается (L346-L353, L365-L373).
        strict: false,
        guard: None,
        halted: false,
        i0,
        i1,
        ng: None,
        native: None,
        input_fee: true,
        pair_d: false,
    })))
}

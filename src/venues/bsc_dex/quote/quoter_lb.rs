//! Файл: venues/bsc_dex/quote/quoter_lb.rs
//! Что делает: квотер сверки пула Liquidity Book V2.1 (решение 290) — сам пул: getSwapOut(amountIn, swapForY) через
//! eth_call (view; тот же цикл по бинам и та же комиссия, что у swap пула, на том же блоке). Ответ (amountInLeft,
//! amountOut, fee): остаток входа больше 0 — swap пула на такую сумму откатился бы (LBPair__OutOfLiquidity), выхода нет.
//! Направление — по порядку X/Y пула из прочитанного состояния (x_is_token0). Вынесено из quoter_calls.rs (предел
//! размера файла).
//! Кто вызывает: quote/quoter_calls.rs (request, decode).
//! От чего зависит: pools/abi_lb.rs, pools/registry.rs, ctx.rs.
//! Настройки: нет (адрес — сам пул).
//!
//! Источник: lfj-gg/joe-v2@v2.1.0 — src/LBPair.sol L424-L468 (getSwapOut), L483-L557 (swap: тот же цикл; остаток входа
//! — откат L538). Факт цепи: на блоке 133716961 наша формула = getSwapOut в 50 из 50 ответов
//! (tests/data/lb_v21_2026-11-15/probe.json).

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256};

use crate::core::types::{PoolRef, PoolState};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::pools::abi_lb::ILBPair as P;
use super::super::pools::registry::Entry;

/// Адрес пула и данные вызова getSwapOut. Принимает: окружение, запись пула, токен входа, сумму. Ошибки: Internal (у
/// пула нет состояния LbBin или ссылка не адрес), NoRoute (сумма больше uint128).
pub fn request(
    ctx: &Ctx,
    e: &Entry,
    token_in: Address,
    amount: U256,
) -> GatewayResult<(Address, Bytes)> {
    let (PoolState::LbBin(d), PoolRef::Contract(pool)) = (&e.pool.state, &e.pool.reference) else {
        return Err(ctx.err(
            ErrorCode::Internal,
            "у пула Liquidity Book не прочитано состояние (порядок X/Y)",
        ));
    };
    let amount: u128 = amount
        .try_into()
        .map_err(|_| ctx.err(ErrorCode::NoRoute, "сумма больше uint128"))?;
    let swap_for_y = (token_in == e.pool.token0) == d.x_is_token0;
    let c = P::getSwapOutCall {
        amountIn: amount,
        swapForY: swap_for_y,
    };
    Ok((*pool, Bytes::from(c.abi_encode())))
}

/// Выход из ответа getSwapOut. None — ответ не по ABI или вход исполняется не целиком (swap откатился бы). Ошибок нет.
pub fn decode(out: &[u8]) -> Option<U256> {
    let r = P::getSwapOutCall::abi_decode_returns(out).ok()?;
    (r.amountInLeft == 0).then_some(U256::from(r.amountOut))
}

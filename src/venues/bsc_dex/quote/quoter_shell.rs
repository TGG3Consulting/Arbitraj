//! Файл: venues/bsc_dex/quote/quoter_shell.rs
//! Что делает: квотер сверки пула Shell Protocol Proteus (решение 486) — сама реализация формулы пула:
//! swapGivenInputAmount(xBalance, yBalance, вход, сторона) — та же функция, которую при обмене зовёт прокси пула
//! (LiquidityPoolProxy.swapGivenInputAmount L49-L61 передаёт балансы ДО обмена). Балансы — прочитанного состояния,
//! вход — в единицах учёта (сумма · 10^(18 − знаков)). Ответ реализации — в единицах учёта; в знаки токена выхода он
//! переводится с decimals() токена выхода (тем же eth_call — пачкой Multicall3 с вызовом реализации) и комиссией
//! вывода учёта прочитанного состояния (как unwrap Ocean). Отдельного квотера у Shell нет.
//! Кто вызывает: quote/quoter_calls.rs (request, decode).
//! От чего зависит: pools/abi_shell.rs, node/multicall.rs, math/proteus_swap.rs, pools/registry.rs, ctx.rs.
//! Настройки: dex.multicall3 [bsc_dex] — без неё сверки нет (NotConfigured).
//!
//! Источник: Sourcify 42161 LiquidityPoolProxy.sol L49-L61, Proteus.sol swapGivenInputAmount (bfbba01 L58-L84);
//! Ocean.sol _erc20Unwrap (aaa0ccf; v3 L726-L741).

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256};

use crate::core::types::{PoolRef, PoolState};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::math::proteus_swap::{from_ocean, to_ocean};
use super::super::node::multicall;
use super::super::pools::abi_shell::{IProteus as R, IShellToken as T};
use super::super::pools::registry::Entry;

/// Адрес Multicall3 и данные пачки «реализация + decimals токена выхода». Принимает: окружение, запись пула, токен
/// входа, сумму (знаки токена). Ошибки: Internal (нет состояния Proteus), NoRoute (токен не из пула, знаков > 18),
/// NotConfigured (dex.multicall3).
pub fn request(
    ctx: &Ctx,
    e: &Entry,
    token_in: Address,
    amount: U256,
) -> GatewayResult<(Address, Bytes)> {
    let (PoolState::Proteus(d), PoolRef::Contract(_)) = (&e.pool.state, &e.pool.reference) else {
        return Err(ctx.err(ErrorCode::Internal, "у пула Shell не прочитано состояние"));
    };
    let in_x = d
        .is_x(token_in)
        .ok_or_else(|| ctx.err(ErrorCode::NoRoute, "Shell: токен входа не из пула"))?;
    let (dec_in, token_out) = if in_x { (d.dec_x, d.token_y) } else { (d.dec_y, d.token_x) };
    let input = to_ocean(amount, dec_in)
        .ok_or_else(|| ctx.err(ErrorCode::NoRoute, "Shell: вход не переводится в единицы учёта"))?;
    let mc = multicall::address(ctx).ok_or_else(|| {
        ctx.not_set("dex.multicall3", "сверка пула Shell — пачкой Multicall3 (реализация и decimals)")
    })?;
    let swap = R::swapGivenInputAmountCall {
        xBalance: d.x_bal,
        yBalance: d.y_bal,
        inputAmount: input,
        inputToken: if in_x { 0 } else { 1 },
    };
    let calls = [
        (d.implementation, Bytes::from(swap.abi_encode())),
        (token_out, Bytes::from(T::decimalsCall {}.abi_encode())),
    ];
    Ok((mc, multicall::encode(&calls)))
}

/// Выход (знаки токена выхода) из ответа пачки. Принимает: запись (делитель комиссии вывода), ответ. Возвращает:
/// выход или None (откат реализации, не по ABI, нет состояния). Ошибок нет.
pub fn decode(e: &Entry, out: &[u8]) -> Option<U256> {
    let PoolState::Proteus(d) = &e.pool.state else {
        return None;
    };
    let got = multicall::decode(out, 2).ok()?;
    let raw = got.first()?.as_ref().ok()?;
    let dec = got.get(1)?.as_ref().ok()?;
    let o = R::swapGivenInputAmountCall::abi_decode_returns(raw).ok()?;
    let dec = T::decimalsCall::abi_decode_returns(dec).ok()?;
    from_ocean(o, dec, d.unwrap_fee_divisor)
}

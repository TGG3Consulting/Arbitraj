//! Файл: venues/bsc_dex/quote/quoter_basin.rs
//! Что делает: квотер сверки скважины Basin (решение 485) — сама скважина: getSwapOut(fromToken, toToken, вход) через
//! eth_call (Well.sol L266-L279: тот же расчёт, что swapFrom, на сохранённых резервах; без проверки предела uint128
//! резерва — её добавляет только запись резервов swapFrom). Отдельной настройки квотера нет: адрес — сама скважина.
//! Кто вызывает: quote/quoter_calls.rs (request, decode).
//! От чего зависит: pools/abi_basin.rs, pools/registry.rs, ctx.rs.
//! Настройки: нет.

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256};

use crate::core::types::{PoolRef, PoolState};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::pools::abi_basin::IBasinWell as W;
use super::super::pools::registry::Entry;

/// Адрес скважины и данные вызова getSwapOut. Принимает: окружение, запись пула, токен входа, сумму. Ошибки: Internal
/// (у пула нет состояния BasinWell или ссылка не адрес), NoRoute (токен входа не из скважины).
pub fn request(
    ctx: &Ctx,
    e: &Entry,
    token_in: Address,
    amount: U256,
) -> GatewayResult<(Address, Bytes)> {
    let (PoolState::BasinWell(d), PoolRef::Contract(well)) = (&e.pool.state, &e.pool.reference)
    else {
        return Err(ctx.err(
            ErrorCode::Internal,
            "у скважины Basin не прочитано состояние",
        ));
    };
    if !d.has(token_in) {
        return Err(ctx.err(ErrorCode::NoRoute, "Basin: токен входа не из скважины"));
    }
    let token_out = if token_in == d.token0 {
        d.token1
    } else {
        d.token0
    };
    let c = W::getSwapOutCall {
        fromToken: token_in,
        toToken: token_out,
        amountIn: amount,
    };
    Ok((*well, Bytes::from(c.abi_encode())))
}

/// Выход из ответа getSwapOut. None — ответ не по ABI. Ошибок нет.
pub fn decode(out: &[u8]) -> Option<U256> {
    W::getSwapOutCall::abi_decode_returns(out).ok()
}

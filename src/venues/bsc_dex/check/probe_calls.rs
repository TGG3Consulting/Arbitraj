//! Файл: venues/bsc_dex/check/probe_calls.rs
//! Что делает: вызовы пробного обмена для eth_simulateV1 — переводы, разрешения и балансы ERC-20.
//! Кто вызывает: check/probe.rs.
//! Там же: откат шага пробной покупки — вердикт по монете или помеха (buy_revert).
//! От чего зависит: check/simulate.rs (Call), node/abi.rs, quote/quoter_revert.rs.
//! Настройки: нет.
//!
//! ERC-20 — https://eips.ethereum.org/EIPS/eip-20 .

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256};

use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::node::abi::IERC20;
use super::super::quote::quoter_revert;
use super::simulate::{Call, CallResult};

/// Вызов from → to с данными. Ошибок нет.
pub fn call(from: Address, to: Address, data: Vec<u8>) -> Call {
    Call {
        from,
        to,
        input: Bytes::from(data),
    }
}

/// transfer токена. Ошибок нет.
pub fn transfer(from: Address, token: Address, to: Address, value: U256) -> Call {
    call(from, token, IERC20::transferCall { to, value }.abi_encode())
}

/// approve токена. Ошибок нет.
pub fn approve(from: Address, token: Address, spender: Address, value: U256) -> Call {
    call(
        from,
        token,
        IERC20::approveCall { spender, value }.abi_encode(),
    )
}

/// balanceOf. Ошибок нет.
pub fn balance(from: Address, token: Address, owner: Address) -> Call {
    call(from, token, IERC20::balanceOfCall { owner }.abi_encode())
}

/// Откат шага пробной покупки: вердикт по монете или помеха окружения. Шаги до квотера (перевод USDT из пула-источника,
/// разрешения на USDT) монету не касаются; откат квотера «не хватает ликвидности» (quote/quoter_revert.rs) — свойство
/// пула на эту сумму. Это — Err NoRoute: проверка не выполнена, монета не в чёрный список.
/// Принимает: номер шага, итог, номер шага квотера. Возвращает: Ok(причина вердикта) или Err.
pub fn buy_revert(ctx: &Ctx, i: usize, r: &CallResult, quoter: usize) -> GatewayResult<String> {
    let why = format!("шаг {i} пробной покупки откатился: {}", r.why());
    let msg = r
        .error
        .as_ref()
        .map(|e| e.message.as_str())
        .unwrap_or_default();
    let env =
        i < quoter || (i == quoter && quoter_revert::classify(msg, Some(&r.return_data)).is_some());
    if env {
        return Err(ctx.err(ErrorCode::NoRoute, why));
    }
    Ok(why)
}

/// uint256 из ответа balanceOf. None — не по ABI.
pub fn u256(r: &CallResult) -> Option<U256> {
    IERC20::balanceOfCall::abi_decode_returns(&r.return_data).ok()
}

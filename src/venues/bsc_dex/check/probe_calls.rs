//! Файл: venues/bsc_dex/check/probe_calls.rs
//! Что делает: вызовы пробного обмена для eth_simulateV1 — переводы и балансы ERC-20, расчёт без налога
//! (роутер V2 getAmountsOut / QuoterV2), хвост пробы: перевод в пул, перевод на пустой адрес, продажа остатка.
//! Кто вызывает: check/probe.rs.
//! От чего зависит: check/simulate.rs (Call), node/abi.rs, node/abi_dex.rs, pools/registry.rs.
//! Настройки: нет.
//!
//! ERC-20 — https://eips.ethereum.org/EIPS/eip-20 ; роутер и квотер — шапка node/abi_dex.rs.

use alloy::sol_types::SolCall;
use alloy_primitives::aliases::{U24, U160};
use alloy_primitives::{Address, Bytes, U256};

use super::super::node::abi::IERC20;
use super::super::node::abi_dex::{IQuoterV2, IV2Router};
use super::super::pools::families::Proto;
use super::super::pools::registry::Entry;
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

/// Расчёт выхода покупки без налога: V2 — роутер getAmountsOut, V3 — QuoterV2. None — у V3 нет квотера.
pub fn expected(
    e: &Entry,
    wallet: Address,
    router: Address,
    quoter: Option<Address>,
    amount: U256,
) -> Option<Call> {
    match (e.family.proto, quoter) {
        (Proto::V2, _) => Some(call(
            wallet,
            router,
            IV2Router::getAmountsOutCall {
                amountIn: amount,
                path: vec![e.usdt, e.coin],
            }
            .abi_encode(),
        )),
        (_, Some(q)) => {
            let p = IQuoterV2::QuoteExactInputSingleParams {
                tokenIn: e.usdt,
                tokenOut: e.coin,
                amountIn: amount,
                fee: U24::from(e.pool.fee_pips.unwrap_or_default()),
                sqrtPriceLimitX96: U160::ZERO,
            };
            Some(call(
                wallet,
                q,
                IQuoterV2::quoteExactInputSingleCall { params: p }.abi_encode(),
            ))
        }
        _ => None,
    }
}

/// Разбор ответа расчёта без налога. None — не по ABI.
pub fn expected_out(e: &Entry, r: &CallResult) -> Option<U256> {
    match e.family.proto {
        Proto::V2 => IV2Router::getAmountsOutCall::abi_decode_returns(&r.return_data)
            .ok()
            .and_then(|v| v.last().copied()),
        _ => IQuoterV2::quoteExactInputSingleCall::abi_decode_returns(&r.return_data)
            .ok()
            .map(|x| x.amountOut),
    }
}

/// uint256 из ответа balanceOf. None — не по ABI.
pub fn u256(r: &CallResult) -> Option<U256> {
    IERC20::balanceOfCall::abi_decode_returns(&r.return_data).ok()
}

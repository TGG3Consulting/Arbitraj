//! Файл: venues/bsc_dex/check/probe_calls.rs
//! Что делает: вызовы пробного обмена для eth_simulateV1 — переводы, разрешения и балансы ERC-20.
//! Кто вызывает: check/probe.rs.
//! От чего зависит: check/simulate.rs (Call), node/abi.rs.
//! Настройки: нет.
//!
//! ERC-20 — https://eips.ethereum.org/EIPS/eip-20 .

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256};

use super::super::node::abi::IERC20;
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

/// uint256 из ответа balanceOf. None — не по ABI.
pub fn u256(r: &CallResult) -> Option<U256> {
    IERC20::balanceOfCall::abi_decode_returns(&r.return_data).ok()
}

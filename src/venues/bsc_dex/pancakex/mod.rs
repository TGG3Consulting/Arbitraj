//! Собственные пределы пользователя из ABI PancakeSwap X и локально рассчитанной подписи Permit2.
//! https://developer.pancakeswap.finance/contracts/pcsx/swap-integration
mod abi;
pub mod assets;
mod book;
pub mod cash;
pub mod client;
pub(in crate::venues::bsc_dex) mod execute;
mod history;
pub(in crate::venues::bsc_dex) mod ledger;
pub mod market;
pub mod profile;
pub(in crate::venues::bsc_dex) mod recovery;
mod result;
mod wire;
use alloy_primitives::{Address, B256, U256};

/// Подтверждённые роли выбранной сети; chain и Permit2 вместе образуют домен подписи.
#[derive(Clone, Copy, Debug)]
pub struct Roles {
    pub chain: u64,
    pub reactor: Address,
    pub permit: Address,
}
/// Владелец проверяет только поручение на свой кошелёк и исходную пару.
#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize)]
pub struct Request {
    pub owner: Address,
    pub input: Address,
    pub output: Address,
    pub amount: U256,
    pub minimum: U256,
    pub exact_output: bool,
    pub now: u64,
}
/// Минимальная гарантированная номинальная выплата. Нетто ERC20 требует проверки его перевода.
#[derive(Clone, Debug)]
pub struct Terms {
    roles: Roles,
    request: Request,
    pub order_hash: B256,
    pub digest: B256,
    pub nonce: U256,
    pub deadline: u64,
    pub min_output: U256,
    pub quoted_output: U256,
    pub maximum_input: U256,
    pub quoted_input: U256,
}
#[cfg(test)]
mod cash_tests;
#[cfg(test)]
mod exact_out_tests;
#[cfg(test)]
mod tests;

#[cfg(test)]
mod lifecycle_tests;

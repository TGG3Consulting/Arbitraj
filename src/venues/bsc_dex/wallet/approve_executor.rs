//! Файл: venues/bsc_dex/wallet/approve_executor.rs
//! Что делает: разрешения кошелька своему исполнителю ArbExecutor (решение 122, часть X): исполнитель забирает вход
//! transferFrom(кошелёк, …) по обычному ERC-20 allowance — как роутер форка, без Permit2; один раз на токен при
//! dex.approve_unlimited = да. Каких не хватает (с узла), все сразу (сухая сделка с подставленным кодом) и чтение
//! allowance для памяти разрешений котировки (wallet/allow_cache.rs::erc20).
//! Кто вызывает: exec/executor/{trade, dry}.rs, wallet/allow_cache.rs.
//! От чего зависит: wallet/approve.rs (fork_missing, fork_all, Approval), node/call.rs, node/abi.rs (IERC20).
//! Настройки: dex.approve_unlimited [bsc_dex] (через approve.rs).
//!
//! ERC-20 allowance / approve — https://eips.ethereum.org/EIPS/eip-20 .

use alloy_primitives::{Address, U256};

use crate::gateway::errors::GatewayResult;

use super::super::ctx::Ctx;
use super::super::node::abi::IERC20;
use super::super::node::call;
use super::approve::{Approval, fork_all, fork_missing};

/// Каких разрешений исполнителю не хватает для списания amount (читает allowance(кошелёк, исполнитель) с узла).
/// Принимает: токен, кошелёк, исполнитель, сумму. Возвращает: вызовы (пусто — хватает). Ошибки: NotConfigured, узел.
pub async fn missing(
    ctx: &Ctx,
    token: Address,
    wallet: Address,
    executor: Address,
    amount: U256,
) -> GatewayResult<Vec<Approval>> {
    fork_missing(ctx, token, wallet, executor, amount).await
}

/// Все разрешения исполнителю сразу (без узла). Ошибки: NotConfigured.
pub fn all(
    ctx: &Ctx,
    token: Address,
    executor: Address,
    amount: U256,
) -> GatewayResult<Vec<Approval>> {
    fork_all(ctx, token, executor, amount)
}

/// ERC-20 allowance(владелец, тратящий) с узла. Ошибки: узел.
pub async fn allowance(
    ctx: &Ctx,
    token: Address,
    owner: Address,
    spender: Address,
) -> GatewayResult<U256> {
    call::view(
        ctx,
        token,
        &IERC20::allowanceCall { owner, spender },
        "latest",
    )
    .await
}

//! Файл: venues/bsc_dex/exec/fork_swap.rs
//! Что делает: чистая подготовка свопа через СОБСТВЕННЫЙ роутер форка (classic V2 Router, решения 59, 82, фаза 80.9b) —
//! у форка нет Universal Router своей фабрики, поэтому нога форка исполняется прямым вызовом
//! router.swapExactTokensForTokensSupportingFeeOnTransferTokens(amountIn, amountOutMin, path, to, deadline). Вариант
//! FeeOnTransfer — проект торгует монетами с налогом: роутер считает реально полученное получателем как ДЕЛЬТУ его
//! баланса (balanceBefore→balanceAfter) и откатывается, если дельта < amountOutMin (решение 40). Получатель — кошелёк,
//! поэтому дельта меряется поверх уже лежащего (кумулятивно, в т.ч. поверх выходов прошлых групп бандла) — отдельный
//! BALANCE_CHECK, как у Universal Router, не нужен. Разрешение — прямой ERC-20 approve(router) (wallet/approve.rs
//! fork_all/fork_missing), без Permit2. Только данные (без сети): сборка бандла/симуляция/подпись — exec/bundle.rs,
//! exec/bundle_sim.rs.
//! Кто вызывает: exec/bundle.rs (сборка ForkSwap), exec/bundle_sim.rs (рендер calldata в симуляции и подписи); тесты.
//! От чего зависит: node/abi_dex.rs (IV2Router), exec/chain.rs (Step/Order), ctx.rs.
//! Настройки: своих нет (dex.router, dex.deadline_s, dex.approve_unlimited читают вызывающие).
//!
//! swapExactTokensForTokensSupportingFeeOnTransferTokens(amountIn, amountOutMin, path[], to, deadline) —
//! IUniswapV2Router02.sol (node/abi_dex.rs шапка); путь V2 — массив адресов токенов, роутер находит пары своей фабрикой.

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256};

use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::node::abi_dex::IV2Router;
use super::super::pools::registry::Entry;
use super::super::wallet::approve::{self, Approval};
use super::chain::Step;

/// Calldata swapExactTokensForTokensSupportingFeeOnTransferTokens. Чистая. Ошибок нет (кодирование).
pub fn swap_data(
    amount_in: U256,
    min_out: U256,
    path: &[Address],
    to: Address,
    deadline: U256,
) -> Bytes {
    Bytes::from(
        IV2Router::swapExactTokensForTokensSupportingFeeOnTransferTokensCall {
            amountIn: amount_in,
            amountOutMin: min_out,
            path: path.to_vec(),
            to,
            deadline,
        }
        .abi_encode(),
    )
}

/// Путь токенов ноги форка из её шагов (exec/chain.rs): вход первого шага, затем выход каждого шага по порядку.
/// Возвращает: [token_in, …, token_out] (≥ 2). Ошибки: NoRoute (нет шагов; выход шага не совпал со входом следующего —
/// такую ногу роутер форка одним вызовом не исполнит).
pub fn token_path(ctx: &Ctx, steps: &[Step]) -> GatewayResult<Vec<Address>> {
    let first = steps
        .first()
        .ok_or_else(|| ctx.err(ErrorCode::NoRoute, "нога форка без шагов"))?;
    let mut path = vec![first.order.token_in, first.order.token_out];
    for s in &steps[1..] {
        if s.order.token_in != *path.last().unwrap() {
            return Err(ctx.err(
                ErrorCode::NoRoute,
                "разрыв пути ноги форка: выход шага не вход следующего — одним вызовом роутера не исполнить",
            ));
        }
        path.push(s.order.token_out);
    }
    Ok(path)
}

/// Вход ноги форка (units) — сумма, которую платит кошелёк (amount_in первого шага). Ошибки: NoRoute (нет шагов).
pub fn amount_in(ctx: &Ctx, steps: &[Step]) -> GatewayResult<U256> {
    Ok(steps
        .first()
        .ok_or_else(|| ctx.err(ErrorCode::NoRoute, "нога форка без шагов"))?
        .order
        .amount_in)
}

/// Пробный своп форка для измерения налога (check/probe.rs): ERC-20 approve роутеру форка (без Permit2) + calldata
/// свопа token_in→token_out с минимумом 1 (проба меряет налог и не должна откатиться из-за минимума; 0 запрещён,
/// exec/plan.rs). Получатель — кошелёк (роутер FeeOnTransfer меряет его дельту). Чистая (без сети). Возвращает:
/// (разрешения, роутер форка, calldata свопа). Ошибки: NotConfigured (dex.router, dex.deadline_s, dex.approve_unlimited).
pub fn probe_swap(
    ctx: &Ctx,
    e: &Entry,
    token_in: Address,
    token_out: Address,
    amount_in: U256,
    wallet: Address,
    now: u64,
) -> GatewayResult<(Vec<Approval>, Address, Bytes)> {
    let router = ctx.dex_addr("dex.router", e.family.id)?;
    let deadline = U256::from(now + ctx.int("dex.deadline_s")?);
    let approvals = approve::fork_all(ctx, token_in, router, amount_in)?;
    let data = swap_data(
        amount_in,
        U256::from(1u8),
        &[token_in, token_out],
        wallet,
        deadline,
    );
    Ok((approvals, router, data))
}

/// Минимум выхода ноги форка (units) — min_out последнего шага (проставлен верификацией, решения 83/84). Ошибки:
/// NoRoute (нет шагов), QuoteMismatch (минимум 0 — обмен без минимума не отправляется, как у Universal Router).
pub fn min_out(ctx: &Ctx, steps: &[Step]) -> GatewayResult<U256> {
    let m = steps
        .last()
        .ok_or_else(|| ctx.err(ErrorCode::NoRoute, "нога форка без шагов"))?
        .order
        .min_out;
    if m.is_zero() {
        return Err(ctx.err(
            ErrorCode::QuoteMismatch,
            "минимум выхода ноги форка 0 — обмен без минимума не отправляется (решение 40)",
        ));
    }
    Ok(m)
}

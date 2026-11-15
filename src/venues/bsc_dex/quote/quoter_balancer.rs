//! Файл: venues/bsc_dex/quote/quoter_balancer.rs
//! Что делает: квотер сверки пула Balancer V2 (решение 289) — само хранилище Vault своей DEX (dex.vault):
//! queryBatchSwap(GIVEN_IN, [шаг: id пула, вход 0 → выход 1, сумма], [tokenIn, tokenOut], funds — нули). Хранилище
//! исполняет обмен с вызовом пула и откатывает, отдавая дельты ответом (eth_call — не транзакция); выход — минус дельта
//! токена выхода. Тот же путь (onSwap пула, те же балансы), что у Vault.swap шага BALANCER исполнителя.
//! Вынесено из quoter_calls.rs (предел размера файла).
//! Кто вызывает: quote/quoter_calls.rs (request, decode).
//! От чего зависит: pools/abi_balancer.rs, pools/registry.rs, ctx.rs.
//! Настройки: dex.vault [DEX вида balancer_v2].
//!
//! Источник: balancer/balancer-deployments@b55dc2b8 v2/tasks/20210418-vault — IVault.sol L587-L593 (BatchSwapStep),
//! L623-L628 (FundManagement), L645-L650 (queryBatchSwap: «performs no checks on the sender or recipient»), Swaps.sol
//! L446-L535. Факт цепи: на блоке 133699740 ответ queryBatchSwap = выходу шага BALANCER исполнителя v2 в 10 из 10
//! случаев (tests/data/balancer_acsi_2026-11-15/exec_sim.json).

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256};

use crate::core::types::PoolState;
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::pools::abi_balancer::IBalancerVault as V;
use super::super::pools::registry::Entry;

/// Данные вызова queryBatchSwap на точный вход. Принимает: id пула, вход, выход, сумму. Ошибок нет.
pub fn data(
    pool_id: alloy_primitives::B256,
    token_in: Address,
    token_out: Address,
    amount: U256,
) -> Bytes {
    let step = V::BatchSwapStep {
        poolId: pool_id,
        assetInIndex: U256::ZERO,
        assetOutIndex: U256::from(1u8),
        amount,
        userData: Bytes::new(),
    };
    let funds = V::FundManagement {
        sender: Address::ZERO,
        fromInternalBalance: false,
        recipient: Address::ZERO,
        toInternalBalance: false,
    };
    // kind 0 = GIVEN_IN (IVault.sol L498).
    let c = V::queryBatchSwapCall {
        kind: 0,
        swaps: vec![step],
        assets: vec![token_in, token_out],
        funds,
    };
    Bytes::from(c.abi_encode())
}

/// Адрес хранилища и данные вызова. Принимает: окружение, запись пула, токен входа, сумму. Ошибки: NotConfigured
/// (dex.vault), Internal (у пула нет состояния VaultPool — id не прочитан).
pub fn request(
    ctx: &Ctx,
    e: &Entry,
    token_in: Address,
    amount: U256,
) -> GatewayResult<(Address, Bytes)> {
    let PoolState::VaultPool(d) = &e.pool.state else {
        return Err(ctx.err(
            ErrorCode::Internal,
            "у пула Balancer не прочитано состояние (id пула)",
        ));
    };
    let token_out = if token_in == e.pool.token0 {
        e.pool.token1
    } else {
        e.pool.token0
    };
    Ok((
        ctx.dex_addr("dex.vault", e.family.id)?,
        data(d.pool_id, token_in, token_out, amount),
    ))
}

/// Выход из ответа queryBatchSwap: минус дельта второго актива (отрицательная — хранилище отдаёт). None — ответ не по
/// ABI, не два актива или дельта выхода не отрицательна. Ошибок нет.
pub fn decode(out: &[u8]) -> Option<U256> {
    let d = V::queryBatchSwapCall::abi_decode_returns(out).ok()?;
    let o = *d.get(1)?;
    (d.len() == 2 && o.is_negative()).then(|| o.unsigned_abs())
}

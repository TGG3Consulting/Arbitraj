//! Файл: venues/bsc_dex/quote/quoter_balancer_v3.rs
//! Что делает: решение 441 (T433-F2) — квотер сверки пула Balancer V3: Router V3 своей DEX (dex.quoter) —
//! querySwapSingleTokenExactIn(пул, вход, выход, сумма, sender = 0, userData пусто). Роутер вызывает Vault.quote, а
//! хранилище — querySwapHook роутера: тот же Vault._swap (onSwap пула, те же балансы, курсы и комиссия), что у
//! Vault.swap шага BALANCER_V3 исполнителя; изменения откатываются (только eth_call без from: Vault.quote требует
//! tx.origin = 0).
//! Кто вызывает: quote/quoter_calls.rs (request, decode).
//! От чего зависит: pools/abi_balancer_v3.rs, pools/registry.rs, ctx.rs.
//! Настройки: dex.quoter [DEX вида balancer_v3].
//!
//! Источник: balancer/balancer-deployments@b55dc2b8 build-info v3/tasks/20250307-v3-router-v2 → contracts/Router.sol
//! L992-L1021 (querySwapSingleTokenExactIn: limit 0, deadline — максимум); 20241204-v3-vault VaultExtension.sol
//! L849-L872 (quote: query — isStaticCall = tx.origin == 0, EVMCallModeHelpers.sol). Факт цепи — запись
//! tools/arbitrum_record/balancer_v3.py (ответ роутера = расчёт math/balancer_v3.rs, тест tests/arbitrum_balancer_v3.rs).

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256};

use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::pools::abi_balancer_v3::IBalancerV3Router as R;
use super::super::pools::registry::Entry;

/// Данные вызова querySwapSingleTokenExactIn. Принимает: пул, вход, выход, сумму. Ошибок нет.
pub fn data(pool: Address, token_in: Address, token_out: Address, amount: U256) -> Bytes {
    let c = R::querySwapSingleTokenExactInCall {
        pool,
        tokenIn: token_in,
        tokenOut: token_out,
        exactAmountIn: amount,
        sender: Address::ZERO,
        userData: Bytes::new(),
    };
    Bytes::from(c.abi_encode())
}

/// Адрес роутера и данные вызова. Принимает: окружение, запись пула, токен входа, сумму. Ошибки: NotConfigured
/// (dex.quoter), Internal (ссылка на пул — не контракт).
pub fn request(
    ctx: &Ctx,
    e: &Entry,
    token_in: Address,
    amount: U256,
) -> GatewayResult<(Address, Bytes)> {
    let Some(pool) = e.pool.reference.own_contract() else {
        return Err(ctx.err(
            ErrorCode::Internal,
            "ссылка на пул Balancer V3 — не адрес контракта",
        ));
    };
    let token_out = if token_in == e.pool.token0 {
        e.pool.token1
    } else {
        e.pool.token0
    };
    Ok((
        ctx.dex_addr("dex.quoter", e.family.id)?,
        data(pool, token_in, token_out, amount),
    ))
}

/// Выход из ответа роутера (amountCalculated — выход на точный вход). None — ответ не по ABI. Ошибок нет.
pub fn decode(out: &[u8]) -> Option<U256> {
    R::querySwapSingleTokenExactInCall::abi_decode_returns(out).ok()
}

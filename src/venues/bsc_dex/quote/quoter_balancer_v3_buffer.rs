//! Файл: venues/bsc_dex/quote/quoter_balancer_v3_buffer.rs
//! Что делает: решение 474 — квотер сверки пары Balancer V3, у которой сторона идёт через буфер ERC4626 хранилища:
//! BatchRouter V3 своей DEX (dex.bv3_batch_router) — querySwapExactIn с одним путём «WRAP в буфере обёртки входа →
//! обмен в пуле → UNWRAP в буфере обёртки выхода» (шаги isBuffer — только у сторон через буфер), sender = 0, userData
//! пусто. Ответ — pathAmountsOut[0]. В режиме запроса хранилище считает буфер по превью обёртки без проверки остатка
//! буфера (Vault.sol L1231, L1352) — тот же выход, что у шага BALANCER_V3_BUF исполнителя при достаточном буфере.
//! eth_call только без from (Vault.quote требует tx.origin = 0).
//! Кто вызывает: quote/quoter_balancer_v3.rs (request, decode).
//! От чего зависит: pools/abi_balancer_v3_buffer.rs, core/types (VaultPoolData, V3Buffer), ctx.rs, pools/registry.rs.
//! Настройки: dex.bv3_batch_router [DEX вида balancer_v3].
//!
//! Источник: balancer/balancer-deployments@b55dc2b8 build-info v3/tasks/20241205-v3-batch-router →
//! contracts/BatchRouter.sol L167-L171 (первый шаг — буфер: токен берётся заранее, кроме статического вызова), L193-L205
//! (шаг isBuffer: UNWRAP, если pool шага = токен входа шага, иначе WRAP; EXACT_IN), L604-L633 (querySwapExactIn:
//! Vault.quote, minAmountOut = 0); IBatchRouter.sol L15-L31, L109-L113.

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256};

use crate::core::types::PoolState;
use crate::core::types::pool_vault::VaultPoolData;
use crate::gateway::errors::GatewayResult;

use super::super::ctx::Ctx;
use super::super::pools::abi_balancer_v3_buffer::IBalancerV3BatchRouter as R;
use super::super::pools::registry::Entry;

/// Обёртки входа и выхода пары по направлению (0 — сторона без буфера). Принимает: состояние пула, вход — token0
/// пары. Возвращает: None — у пары нет стороны через буфер. Ошибок нет.
pub fn wrappers(d: &VaultPoolData, zero_in: bool) -> Option<(Address, Address)> {
    let v3 = d.v3.as_ref()?;
    let (i, j) = if zero_in { (d.i0, d.i1) } else { (d.i1, d.i0) };
    let w = |k: u8| {
        v3.via
            .get(k as usize)
            .and_then(|b| b.as_ref())
            .map_or(Address::ZERO, |b| b.wrapped)
    };
    let (a, b) = (w(i), w(j));
    (!a.is_zero() || !b.is_zero()).then_some((a, b))
}

/// Есть ли у записи пары сторона через буфер. Принимает: запись пула. Ошибок нет.
pub fn buffered(e: &Entry) -> bool {
    matches!(&e.pool.state, PoolState::VaultPool(d) if wrappers(d, true).is_some())
}

/// Данные querySwapExactIn. Принимает: пул, вход, выход пары, обёртки входа / выхода (0 — без буфера), сумму. Ошибок
/// нет.
pub fn data(
    pool: Address,
    (token_in, token_out): (Address, Address),
    (w_in, w_out): (Address, Address),
    amount: U256,
) -> Bytes {
    let step = |pool: Address, out: Address, buf: bool| R::SwapPathStep {
        pool,
        tokenOut: out,
        isBuffer: buf,
    };
    let mut steps = Vec::new();
    if !w_in.is_zero() {
        steps.push(step(w_in, w_in, true));
    }
    let pool_out = if w_out.is_zero() { token_out } else { w_out };
    steps.push(step(pool, pool_out, false));
    if !w_out.is_zero() {
        steps.push(step(w_out, token_out, true));
    }
    let path = R::SwapPathExactAmountIn {
        tokenIn: token_in,
        steps,
        exactAmountIn: amount,
        minAmountOut: U256::ZERO,
    };
    let c = R::querySwapExactInCall {
        paths: vec![path],
        sender: Address::ZERO,
        userData: Bytes::new(),
    };
    Bytes::from(c.abi_encode())
}

/// Адрес BatchRouter и данные вызова. Принимает: окружение, запись пары, пул, вход, сумму. Возвращает: None — у пары
/// нет стороны через буфер. Ошибки: NotConfigured (dex.bv3_batch_router).
pub fn request(
    ctx: &Ctx,
    e: &Entry,
    pool: Address,
    token_in: Address,
    amount: U256,
) -> Option<GatewayResult<(Address, Bytes)>> {
    let PoolState::VaultPool(d) = &e.pool.state else {
        return None;
    };
    let zero_in = token_in == e.pool.token0;
    let ws = wrappers(d, zero_in)?;
    let token_out = if zero_in {
        e.pool.token1
    } else {
        e.pool.token0
    };
    Some(
        ctx.dex_addr("dex.bv3_batch_router", e.family.id)
            .map(|r| (r, data(pool, (token_in, token_out), ws, amount))),
    )
}

/// Выход из ответа BatchRouter (pathAmountsOut[0]). None — не по ABI или путей нет. Ошибок нет.
pub fn decode(out: &[u8]) -> Option<U256> {
    let r = R::querySwapExactInCall::abi_decode_returns(out).ok()?;
    r.pathAmountsOut.first().copied()
}

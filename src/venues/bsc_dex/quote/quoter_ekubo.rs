//! Файл: venues/bsc_dex/quote/quoter_ekubo.rs
//! Что делает: квотер сверки пула Ekubo (решение 447) — Router.quote официального MEVCaptureRouter (dex.quoter):
//! обмен в ядре под lock с откатом (lockAndExpectRevert), пулы с MEVCapture — через forward к расширению, ответ —
//! (balanceUpdate, stateAfter). Вызов — точный вход: amount > 0, вход token1 — isToken1, предел цены 0 (роутер ставит
//! MIN / MAX_SQRT_RATIO — withDefaultSqrtRatioLimit), skipAhead = 0 — как шаг EKUBO исполнителя (те же шаги обмена,
//! те же округления). Выход — отрицательная дельта пула по выходному токену.
//! Кто вызывает: quote/quoter_calls.rs (request, decode).
//! От чего зависит: pools/abi_ekubo.rs, pools/registry.rs, ctx.rs.
//! Настройки: dex.quoter [DEX] — адрес MEVCaptureRouter (docs.ekubo.org evm-v3: 0xd26f20001a72a18C002b00e6710000d68700ce00).
//!
//! Источник — EkuboProtocol/evm-contracts@v3.1.1 (сборка MEVCaptureRouter, docs.ekubo.org evm-v3 «legacy, v3.1.1»):
//! src/Router.sol L509-L552 (quote), src/MEVCaptureRouter.sol L27-L44 (_swap: MEVCapture — forward);
//! @61d09daa src/types/swapParameters.sol L271-L279 (withDefaultSqrtRatioLimit), src/types/poolBalanceUpdate.sol L7-L17.

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256};

use crate::core::types::PoolState;
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::pools::abi_ekubo::{EkuboPoolKey, IEkuboRouter as R, decode_balance};
use super::super::pools::registry::Entry;

/// Адрес роутера и данные вызова quote. Принимает: окружение, запись пула, токен входа, сумму.
/// Ошибки: Internal (у пула нет состояния Ekubo), NoRoute (сумма больше int128), NotConfigured (dex.quoter).
pub fn request(
    ctx: &Ctx,
    e: &Entry,
    token_in: Address,
    amount: U256,
) -> GatewayResult<(Address, Bytes)> {
    let PoolState::Ekubo(d) = &e.pool.state else {
        return Err(ctx.err(ErrorCode::Internal, "у пула Ekubo не прочитано состояние"));
    };
    let amount: i128 = amount
        .try_into()
        .map_err(|_| ctx.err(ErrorCode::NoRoute, "сумма больше int128"))?;
    let key = EkuboPoolKey {
        token0: e.pool.token0,
        token1: e.pool.token1,
        config: d.config,
    };
    let c = R::quoteCall {
        poolKey: key,
        isToken1: token_in == e.pool.token1,
        amount,
        sqrtRatioLimit: Default::default(),
        skipAhead: U256::ZERO,
    };
    let router = ctx.dex_addr("dex.quoter", e.family.id)?;
    Ok((router, Bytes::from(c.abi_encode())))
}

/// Выход из ответа quote: отрицательная дельта пула (пул отдал). None — ответ не по ABI или пул ничего не отдал.
/// Ошибок нет.
pub fn decode(out: &[u8]) -> Option<U256> {
    let r = R::quoteCall::abi_decode_returns(out).ok()?;
    let (d0, d1) = decode_balance(&r.balanceUpdate);
    let paid = d0.min(d1);
    (paid < 0).then(|| U256::from(paid.unsigned_abs()))
}

//! Файл: venues/bsc_dex/quote/quoter_calls.rs
//! Что делает: данные вызова квотера своей DEX на точный вход и разбор ответа — одна точка для eth_call
//! (quote/quoter.rs) и для пробного обмена в симуляции (check/probe.rs).
//! V2 — Router.getAmountsOut; V3 — QuoterV2.quoteExactInputSingle; Infinity CL/Bin — CLQuoter/BinQuoter;
//! Uniswap V4 — V4Quoter (ключ пула, направление, сумма, hookData пустые).
//! Кто вызывает: quote/quoter.rs, check/probe.rs.
//! От чего зависит: node/abi_dex.rs, pools/key.rs, pools/registry.rs, ctx.rs.
//! Настройки: dex.router (V2), dex.quoter (V3, V4) [вид пула].
//!
//! Сигнатуры — шапка node/abi_dex.rs (официальные интерфейсы Uniswap v2/v3, pancake-v3, infinity-periphery, v4-periphery).

use alloy::sol_types::SolCall;
use alloy_primitives::aliases::{I24, U24, U160};
use alloy_primitives::{Address, Bytes, U256};

use crate::core::types::PoolKind;
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::node::abi_dex::{IInfinityQuoter, IQuoterV2, IV2Router, IV4Quoter};
use super::super::pools::key::PoolKey;
use super::super::pools::registry::Entry;

/// Адрес и данные вызова квотера. Ошибки: NotConfigured (адрес), NoRoute (сумма больше uint128 у V4), Internal (нет ключа).
pub fn request(
    ctx: &Ctx,
    e: &Entry,
    token_in: Address,
    amount: U256,
) -> GatewayResult<(Address, Bytes)> {
    let token_out = if token_in == e.pool.token0 {
        e.pool.token1
    } else {
        e.pool.token0
    };
    let zero_for_one = token_in == e.pool.token0;
    let big = || ctx.err(ErrorCode::NoRoute, "сумма больше uint128");
    let data = match (e.pool.kind, &e.key) {
        (PoolKind::V2, _) => {
            let c = IV2Router::getAmountsOutCall {
                amountIn: amount,
                path: vec![token_in, token_out],
            };
            return Ok((
                ctx.dex_addr("dex.router", e.family.id)?,
                Bytes::from(c.abi_encode()),
            ));
        }
        (PoolKind::V3, _) => {
            let fee = U24::from(e.pool.fee_pips.unwrap_or_default());
            let p = IQuoterV2::QuoteExactInputSingleParams {
                tokenIn: token_in,
                tokenOut: token_out,
                amountIn: amount,
                fee,
                sqrtPriceLimitX96: U160::ZERO,
            };
            IQuoterV2::quoteExactInputSingleCall { params: p }.abi_encode()
        }
        (_, Some(PoolKey::Infinity(k))) => {
            let key = IInfinityQuoter::PoolKey {
                currency0: k.currency0,
                currency1: k.currency1,
                hooks: k.hooks,
                poolManager: k.manager,
                fee: U24::from(k.fee),
                parameters: k.parameters,
            };
            let p = IInfinityQuoter::QuoteExactSingleParams {
                poolKey: key,
                zeroForOne: zero_for_one,
                exactAmount: amount.try_into().map_err(|_| big())?,
                hookData: Bytes::new(),
            };
            IInfinityQuoter::quoteExactInputSingleCall { params: p }.abi_encode()
        }
        (_, Some(PoolKey::Uniswap(k))) => {
            let spacing = I24::try_from(k.tick_spacing)
                .map_err(|_| ctx.err(ErrorCode::BadResponse, "tickSpacing вне int24"))?;
            let key = IV4Quoter::PoolKey {
                currency0: k.currency0,
                currency1: k.currency1,
                fee: U24::from(k.fee),
                tickSpacing: spacing,
                hooks: k.hooks,
            };
            let p = IV4Quoter::QuoteExactSingleParams {
                poolKey: key,
                zeroForOne: zero_for_one,
                exactAmount: amount.try_into().map_err(|_| big())?,
                hookData: Bytes::new(),
            };
            IV4Quoter::quoteExactInputSingleCall { params: p }.abi_encode()
        }
        _ => {
            return Err(ctx.err(
                ErrorCode::Internal,
                format!("{}: у пула V4 нет ключа", e.family.title),
            ));
        }
    };
    Ok((ctx.dex_addr("dex.quoter", e.family.id)?, Bytes::from(data)))
}

/// Выход из ответа квотера. None — ответ не по ABI.
pub fn decode(e: &Entry, out: &[u8]) -> Option<U256> {
    match (e.pool.kind, &e.key) {
        (PoolKind::V2, _) => IV2Router::getAmountsOutCall::abi_decode_returns(out)
            .ok()?
            .last()
            .copied(),
        (PoolKind::V3, _) => IQuoterV2::quoteExactInputSingleCall::abi_decode_returns(out)
            .ok()
            .map(|r| r.amountOut),
        (_, Some(PoolKey::Infinity(_))) => {
            IInfinityQuoter::quoteExactInputSingleCall::abi_decode_returns(out)
                .ok()
                .map(|r| r.amountOut)
        }
        (_, Some(PoolKey::Uniswap(_))) => {
            IV4Quoter::quoteExactInputSingleCall::abi_decode_returns(out)
                .ok()
                .map(|r| r.amountOut)
        }
        _ => None,
    }
}

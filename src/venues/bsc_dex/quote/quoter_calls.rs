//! Файл: venues/bsc_dex/quote/quoter_calls.rs
//! Что делает: данные вызова квотера своей DEX на точный вход и разбор ответа — одна точка для eth_call
//! (quote/quoter.rs) и для пробного обмена в симуляции (check/probe.rs).
//! V2 — Router.getAmountsOut; V3 — QuoterV2.quoteExactInputSingle; Infinity CL/Bin — CLQuoter/BinQuoter;
//! Uniswap V4 — V4Quoter (ключ пула, направление, сумма, hookData пустые); KyberSwap Elastic — свой QuoterV2
//! (tokenIn, tokenOut, amountIn, feeUnits, limitSqrtP; решение 81).
//! Кто вызывает: quote/quoter.rs, check/probe.rs.
//! От чего зависит: node/abi_dex.rs, pools/key.rs, pools/registry.rs, ctx.rs.
//! Настройки: dex.router (V2), dex.quoter (V3, V4) [вид пула].
//!
//! Сигнатуры — шапка node/abi_dex.rs (официальные интерфейсы Uniswap v2/v3, pancake-v3, infinity-periphery, v4-periphery).

use alloy::sol_types::SolCall;
use alloy_primitives::aliases::{U24, U160};
use alloy_primitives::{Address, Bytes, U256};

use crate::core::types::{PoolKind, PoolRef, PoolState};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::node::abi_dex::{
    IElasticQuoterV2, IInfinityQuoter, IQuoterV2, IV2Router, IV4Quoter,
};
use super::super::pools::abi_dodo::IDodoPool;
use super::super::pools::families::Proto;
use super::super::pools::key::PoolKey;
use super::super::pools::load_dodo::FEE_TRADER;
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
    // DODO PMM (решение 86b): квотер — сам пул (querySellBase/querySellQuote; отдельного квотера нет). Сверка
    // идёт тем же трейдером FEE_TRADER, что и чтение комиссии getUserFeeRate (load_dodo.rs) — выход включает
    // те же lp/mt комиссии, что и наш локальный net_after_fees. Направление: отдаём базовую монету →
    // querySellBase, иначе querySellQuote. DVMTrader/DPPTrader/DSPTrader (DODOEX/contractV2, решение 23).
    if e.family.proto.is_dodo() {
        let PoolRef::Contract(pool) = e.pool.reference else {
            return Err(ctx.err(ErrorCode::Internal, "ссылка на пул DODO не того вида"));
        };
        let PoolState::Pmm(d) = &e.pool.state else {
            return Err(ctx.err(ErrorCode::Internal, "состояние пула DODO не Pmm"));
        };
        let base = if d.base_is_token0 {
            e.pool.token0
        } else {
            e.pool.token1
        };
        let data = if token_in == base {
            IDodoPool::querySellBaseCall {
                trader: FEE_TRADER,
                payBaseAmount: amount,
            }
            .abi_encode()
        } else {
            IDodoPool::querySellQuoteCall {
                trader: FEE_TRADER,
                payQuoteAmount: amount,
            }
            .abi_encode()
        };
        return Ok((pool, Bytes::from(data)));
    }
    // KyberSwap Elastic (решение 81): свой QuoterV2.quoteExactInputSingle(tokenIn, tokenOut, amountIn, feeUnits,
    // limitSqrtP) — вид ядра Cl без ключа, отдельная ветка по протоколу.
    if e.family.proto == Proto::KyberElastic {
        let fee = U24::from(
            e.pool
                .fee_pips
                .ok_or_else(|| ctx.err(ErrorCode::Internal, "у пула Elastic нет feeUnits"))?,
        );
        let p = IElasticQuoterV2::QuoteExactInputSingleParams {
            tokenIn: token_in,
            tokenOut: token_out,
            amountIn: amount,
            feeUnits: fee,
            limitSqrtP: U160::ZERO,
        };
        return Ok((
            ctx.dex_addr("dex.quoter", e.family.id)?,
            Bytes::from(IElasticQuoterV2::quoteExactInputSingleCall { params: p }.abi_encode()),
        ));
    }
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
            let fee = U24::from(
                e.pool
                    .fee_pips
                    .ok_or_else(|| ctx.err(ErrorCode::Internal, "у пула V3 нет fee"))?,
            );
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
            let spacing = k.tick_spacing;
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
    // DODO PMM (решение 86b): querySellBase/querySellQuote возвращают первым словом выход после комиссий
    // (DVM — 2 значения, DPP/DSP — 4; первое всегда receive*Amount). Читаем первое слово, число значений не важно.
    if e.family.proto.is_dodo() {
        return (out.len() >= 32).then(|| U256::from_be_slice(&out[..32]));
    }
    if e.family.proto == Proto::KyberElastic {
        return IElasticQuoterV2::quoteExactInputSingleCall::abi_decode_returns(out)
            .ok()
            .map(|r| r.returnedAmount);
    }
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

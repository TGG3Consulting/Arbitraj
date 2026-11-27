//! Deployed HotProxy, CrocQuery and CrocImpact. Pool/payee cash flows use signed int128.
//! https://docs.ambient.finance/developers/dex-contract-interface/swaps/hot-path-swap-migration
#[path = "ambient_format.rs"]
mod format;
#[path = "ambient_roles.rs"]
mod deployment;

use super::{balancer_v1::{data, views}, registry::Entry};
pub(crate) use deployment::{Roles, bind};
use format::{MAX_LIMIT_PRICE, MIN_LIMIT_PRICE};
use crate::{
    core::types::{
        LadderPoint, PoolRef, PoolState,
        ambient::{Market, State},
    },
    gateway::errors::{ErrorCode, GatewayError, GatewayResult},
    venues::bsc_dex::{ctx::Ctx, node::call},
};
use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256};
use serde_json::Value;
#[allow(clippy::too_many_arguments)]
mod abi {
    alloy::sol! {
        interface ICroc{
            function dex_() external view returns(address);
            function readSlot(uint256 slot) external view returns(uint256);
            function queryPoolParams(address base,address quote,uint256 index) external view returns(uint8 schema,uint16 feeRate,uint8 protocolTake,uint16 tickSize,uint8 jit,uint8 knockout,uint8 oracleFlags);
            function queryPrice(address base,address quote,uint256 index) external view returns(uint128);
            function calcImpact(address base,address quote,uint256 index,bool buy,bool inBase,uint128 qty,uint16 tip,uint128 limit) external view returns(int128 baseFlow,int128 quoteFlow,uint128 finalPrice);
        }
    }
}
pub(crate) use abi::ICroc;
pub(super) fn bad(ctx: &Ctx) -> GatewayError {
    ctx.err(
        ErrorCode::QuoteMismatch,
        "Ambient: реестр, цена или исполненные количества не подтверждены",
    )
}
pub(super) fn params(query: Address, m: Market) -> (Address, Bytes) {
    data(
        query,
        ICroc::queryPoolParamsCall {
            base: m.base,
            quote: m.quote,
            index: m.index,
        },
    )
}
pub(super) async fn market(ctx: &Ctx, r: Roles, m: Market, at: &Value) -> GatewayResult<()> {
    let calls = [
        params(r.query, m),
        data(
            r.query,
            ICroc::queryPriceCall {
                base: m.base,
                quote: m.quote,
                index: m.index,
            },
        ),
    ];
    let v = views(ctx, &calls, at).await?;
    let [params, price] = v.as_slice() else {
        return Err(bad(ctx));
    };
    let p = call::decode::<ICroc::queryPoolParamsCall>(params)?;
    if p.schema != 1 || p.tickSize == 0 || call::decode::<ICroc::queryPriceCall>(price)? == 0 {
        return Err(bad(ctx));
    }
    Ok(())
}
pub(crate) fn shape(e: &Entry) -> Option<&State> {
    let PoolState::Ambient(s) = &e.pool.state else {
        return None;
    };
    let PoolRef::Singleton { manager, id } = e.pool.reference else {
        return None;
    };
    let coins = s.market.tokens(s.wrapped)?;
    (e.family.variant.ambient()
        && e.pool.kind == crate::core::types::PoolKind::V2
        && manager == s.dex
        && id == s.market.id()
        && !s.dex.is_zero()
        && !s.impact.is_zero()
        && coins.contains(&e.pool.token0)
        && coins.contains(&e.pool.token1)
        && e.pool.token0 != e.pool.token1)
        .then_some(s)
}
pub(crate) async fn check(ctx: &Ctx, e: &Entry, at: &Value) -> GatewayResult<Roles> {
    let s = shape(e).ok_or_else(|| bad(ctx))?;
    let r = bind(ctx, e.family, at).await?;
    if r.dex != s.dex || r.impact != s.impact || ctx.addr("dex.native_wrapped")? != s.wrapped {
        return Err(bad(ctx));
    }
    market(ctx, r, s.market, at).await?;
    Ok(r)
}
pub(crate) fn source(
    impact: Address,
    m: Market,
    buy: bool,
    exact_in: bool,
    qty: U256,
) -> Option<(Address, Bytes)> {
    let qty = u128::try_from(qty)
        .ok()
        .filter(|&x| x > 0 && x <= i128::MAX as u128)?;
    Some(data(
        impact,
        ICroc::calcImpactCall {
            base: m.base,
            quote: m.quote,
            index: m.index,
            buy,
            inBase: buy == exact_in,
            qty,
            tip: 0,
            limit: if buy { MAX_LIMIT_PRICE } else { MIN_LIMIT_PRICE },
        },
    ))
}
pub(crate) fn decode(raw: &[u8], buy: bool) -> Option<LadderPoint> {
    if raw.len() != 96 {
        return None;
    }
    let v = ICroc::calcImpactCall::abi_decode_returns(raw).ok()?;
    let (debit, credit) = if buy {
        (v.baseFlow, v.quoteFlow)
    } else {
        (v.quoteFlow, v.baseFlow)
    };
    (debit > 0 && credit < 0 && v.finalPrice > 0).then(|| LadderPoint {
        amount_in: U256::from(debit.unsigned_abs()),
        amount_out: U256::from(credit.unsigned_abs()),
    })
}
pub(crate) async fn official(
    ctx: &Ctx,
    e: &Entry,
    input: Address,
    amount: U256,
    exact_in: bool,
    at: &Value,
) -> GatewayResult<LadderPoint> {
    let s = shape(e).ok_or_else(|| bad(ctx))?;
    let r = check(ctx, e, at).await?;
    let buy = s.side(input).ok_or_else(|| bad(ctx))? == 0;
    let q = source(r.impact, s.market, buy, exact_in, amount).ok_or_else(|| bad(ctx))?;
    let rows = views(ctx, &[q], at).await?;
    rows.first()
        .and_then(|raw| decode(raw, buy))
        .filter(|p| {
            if exact_in {
                p.amount_in <= amount
            } else {
                p.amount_out >= amount
            }
        })
        .ok_or_else(|| bad(ctx))
}

pub(crate) fn request(
    ctx: &Ctx,
    e: &Entry,
    input: Address,
    cap: U256,
) -> GatewayResult<(Address, Bytes)> {
    let s = shape(e).ok_or_else(|| bad(ctx))?;
    let buy = s.side(input).ok_or_else(|| bad(ctx))? == 0;
    source(s.impact, s.market, buy, true, cap).ok_or_else(|| bad(ctx))
}
pub(crate) fn payout(e: &Entry, input: Address, cap: U256, raw: &[u8]) -> Option<U256> {
    let s = shape(e)?;
    let p = decode(raw, s.side(input)? == 0)?;
    (p.amount_in == cap).then_some(p.amount_out)
}

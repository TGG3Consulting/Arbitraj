//! Формирует запросы к родному resolver для котировки, серии объёмов и нескольких рынков.
use super::{
    balancer_v1::{data, individual, views},
    fluid_dex_lite::{DexKey, IFluidLite, Roles, bad, check, shape},
    registry::Entry,
};
use crate::{
    core::types::{LadderPoint, fluid_dex_lite::Market},
    gateway::errors::GatewayResult,
    venues::bsc_dex::{ctx::Ctx, node::call},
};
use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, I256, U256};
use serde_json::Value;

fn source(
    resolver: Address,
    m: Market,
    forward: bool,
    qty: U256,
    exact_in: bool,
) -> Option<(Address, Bytes)> {
    if qty.is_zero() {
        return None;
    }
    let raw = I256::try_from(qty).ok()?;
    let specified = if exact_in { raw } else { raw.checked_neg()? };
    Some(data(
        resolver,
        IFluidLite::estimateSwapSingleCall {
            pool: DexKey {
                token0: m.token0,
                token1: m.token1,
                salt: m.salt,
            },
            forward,
            specified,
        },
    ))
}
fn point(bytes: &Bytes, qty: U256, exact_in: bool) -> Option<LadderPoint> {
    let other = IFluidLite::estimateSwapSingleCall::abi_decode_returns_validate(bytes).ok()?;
    if other.is_zero() {
        return None;
    }
    let (amount_in, amount_out) = if exact_in { (qty, other) } else { (other, qty) };
    Some(LadderPoint {
        amount_in,
        amount_out,
    })
}
pub(crate) async fn official(
    c: &Ctx,
    e: &Entry,
    input: Address,
    qty: U256,
    exact_in: bool,
    at: &Value,
) -> GatewayResult<LadderPoint> {
    let s = shape(e).ok_or_else(|| bad(c))?;
    let r = check(c, e, at).await?;
    let forward = s.side(input).ok_or_else(|| bad(c))? == 0;
    let call = source(r.resolver, s.market, forward, qty, exact_in).ok_or_else(|| bad(c))?;
    let raw = views(c, &[call], at).await?;
    raw.first()
        .and_then(|b| point(b, qty, exact_in))
        .ok_or_else(|| bad(c))
}
pub(super) async fn levels(
    c: &Ctx,
    r: Roles,
    m: Market,
    forward: bool,
    inputs: &[U256],
    at: &Value,
) -> GatewayResult<Vec<LadderPoint>> {
    let qs: Vec<_> = inputs
        .iter()
        .filter_map(|&n| Some((n, source(r.resolver, m, forward, n, true)?)))
        .collect();
    let calls: Vec<_> = qs.iter().map(|(_, q)| q.clone()).collect();
    let rows = individual(c, &calls, at).await?;
    if rows.len() != qs.len() {
        return Err(bad(c));
    }
    let mut points: Vec<_> = qs
        .into_iter()
        .zip(rows)
        .filter_map(|((n, _), raw)| point(&raw.ok()?, n, true))
        .collect();
    points.sort_by_key(|p| p.amount_in);
    points.dedup_by_key(|p| p.amount_in);
    Ok(points)
}
pub(crate) fn request(
    c: &Ctx,
    e: &Entry,
    input: Address,
    amount: U256,
) -> GatewayResult<(Address, Bytes)> {
    let s = shape(e).ok_or_else(|| bad(c))?;
    source(
        s.resolver,
        s.market,
        s.side(input).ok_or_else(|| bad(c))? == 0,
        amount,
        true,
    )
    .ok_or_else(|| bad(c))
}
pub(crate) fn payout(e: &Entry, input: Address, amount: U256, raw: &[u8]) -> Option<U256> {
    let s = shape(e)?;
    s.side(input)?;
    if amount.is_zero() {
        return None;
    }
    IFluidLite::estimateSwapSingleCall::abi_decode_returns_validate(raw)
        .ok()
        .filter(|v| !v.is_zero())
}

pub(crate) async fn hop_quote(
    ctx: &Ctx,
    resolver: Address,
    path: Vec<Address>,
    markets: &[Market],
    amount: U256,
    exact_in: bool,
    at: &Value,
) -> GatewayResult<U256> {
    let keys = markets
        .iter()
        .map(|m| DexKey {
            token0: m.token0,
            token1: m.token1,
            salt: m.salt,
        })
        .collect();
    let qty = I256::try_from(amount).map_err(|_| bad(ctx))?;
    let specified = if exact_in {
        qty
    } else {
        qty.checked_neg().ok_or_else(|| bad(ctx))?
    };
    call::canonical(
        ctx,
        at,
        resolver,
        IFluidLite::estimateSwapHopCall {
            path,
            keys,
            specified,
        },
    )
    .await
}

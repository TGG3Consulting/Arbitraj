//! Sample original LLAMMA band quotes on a single head. A capped quote stores the actual debit;
//! this preserves narrow markets and prevents the residual principal counting as paid inventory.
use super::{
    balancer_v1::{data, individual, views},
    families::Family,
    ladder_counter,
    llamma::{self as l, ILlama},
    registry::{Entry, Registry},
};
use crate::{
    core::types::{LadderPoint, PoolRef, PoolState, llamma::State},
    gateway::errors::GatewayResult,
    venues::bsc_dex::{ctx::Ctx, node::call, tokens::Tokens},
};
use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256};
use serde_json::Value;
fn source(pool: Address, side: usize, qty: U256) -> (Address, Bytes) {
    data(
        pool,
        ILlama::get_dxdyCall {
            i: U256::from(side),
            j: U256::from(1 - side),
            qty,
        },
    )
}
fn decode(raw: &[u8], cap: U256) -> Option<LadderPoint> {
    if raw.len() != 64 {
        return None;
    }
    let v = ILlama::get_dxdyCall::abi_decode_returns(raw).ok()?;
    (!v.spent.is_zero() && v.spent <= cap && !v.received.is_zero()).then_some(LadderPoint {
        amount_in: v.spent,
        amount_out: v.received,
    })
}
pub(crate) fn request(
    ctx: &Ctx,
    e: &Entry,
    input: Address,
    cap: U256,
) -> GatewayResult<(Address, Bytes)> {
    let s = l::shape(e).ok_or_else(|| l::bad(ctx))?;
    let side = s.side(input).ok_or_else(|| l::bad(ctx))?;
    Ok(source(s.amm, side, cap))
}
pub(crate) fn payout(e: &Entry, input: Address, cap: U256, raw: &[u8]) -> Option<U256> {
    l::shape(e)?.side(input)?;
    Some(decode(raw, cap)?.amount_out)
}
pub(crate) async fn official(
    ctx: &Ctx,
    e: &Entry,
    input: Address,
    qty: U256,
    by_target: bool,
    at: &Value,
) -> GatewayResult<LadderPoint> {
    l::check(ctx, e, at).await?;
    let s = l::shape(e).ok_or_else(|| l::bad(ctx))?;
    let side = s.side(input).ok_or_else(|| l::bad(ctx))?;
    if by_target {
        let req = data(
            s.amm,
            ILlama::get_dydxCall {
                i: U256::from(side),
                j: U256::from(1 - side),
                qty,
            },
        );
        let rows = views(ctx, &[req], at).await?;
        let v = call::decode::<ILlama::get_dydxCall>(&rows[0])?;
        if v.received != qty || qty.is_zero() || v.spent.is_zero() {
            return Err(l::bad(ctx));
        }
        return Ok(LadderPoint {
            amount_in: v.spent,
            amount_out: v.received,
        });
    }
    let rows = views(ctx, &[source(s.amm, side, qty)], at).await?;
    decode(&rows[0], qty).ok_or_else(|| l::bad(ctx))
}
async fn levels(
    ctx: &Ctx,
    pool: Address,
    side: usize,
    sizes: &[U256],
    at: &Value,
) -> GatewayResult<(Vec<LadderPoint>, Option<U256>)> {
    let calls = sizes
        .iter()
        .map(|&n| source(pool, side, n))
        .collect::<Vec<_>>();
    let rows = individual(ctx, &calls, at).await?;
    let mut points = Vec::new();
    let mut partial = None;
    for (&cap, row) in sizes.iter().zip(rows) {
        let Some(p) = row.ok().and_then(|b| decode(&b, cap)) else {
            continue;
        };
        if p.amount_in < cap {
            partial = Some(partial.map_or(cap, |old: U256| old.max(cap)))
        }
        points.push(p);
    }
    points.sort_unstable_by_key(|p| p.amount_in);
    if points
        .windows(2)
        .any(|w| w[0].amount_in == w[1].amount_in && w[0].amount_out != w[1].amount_out)
    {
        return Err(l::bad(ctx));
    }
    points.dedup_by_key(|p| p.amount_in);
    Ok((points, partial))
}
#[allow(clippy::too_many_arguments)]
pub(crate) async fn load(
    ctx: &Ctx,
    tokens: &Tokens,
    reg: &Registry,
    f: &Family,
    r: &PoolRef,
    pair: (Address, Address),
    counter: Address,
    at: &Value,
) -> GatewayResult<PoolState> {
    let PoolRef::Singleton { manager, id } = r else {
        return Err(l::bad(ctx));
    };
    let (index, amm) = l::decode_key(*id).ok_or_else(|| l::bad(ctx))?;
    let (factory, m, runtime) = l::bound(ctx, f, index, at).await?;
    if amm != *manager
        || amm != m.amm
        || !(m.coins == [pair.0, pair.1] || m.coins == [pair.1, pair.0])
    {
        return Err(l::bad(ctx));
    }
    let side = m
        .coins
        .iter()
        .position(|a| *a == counter)
        .ok_or_else(|| l::bad(ctx))?;
    let (buys, mut sells) = tokio::try_join!(
        ladder_counter::amounts(ctx, tokens, reg, f.id, counter),
        ladder_counter::amounts(ctx, tokens, reg, f.id, m.coins[1 - side])
    )?;
    let buy = levels(ctx, amm, side, &buys, at).await?;
    sells.extend(buy.0.iter().map(|p| p.amount_out));
    sells.sort_unstable();
    sells.dedup();
    let sell = levels(ctx, amm, 1 - side, &sells, at).await?;
    let [(a, pa), (b, pb)] = if side == 0 { [buy, sell] } else { [sell, buy] };
    Ok(PoolState::Llamma(Box::new(State {
        factory,
        index,
        amm,
        runtime,
        controller: m.controller,
        coins: m.coins,
        levels: [a, b],
        partial_cap: [pa, pb],
    })))
}

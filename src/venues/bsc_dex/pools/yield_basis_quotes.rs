//! Independent original get_dy quotes. Native original execution always requires funded verification.
use super::{
    balancer_v1::{data, views},
    families::Family,
    ladder_counter, load_metric,
    registry::{Entry, Registry},
    yield_basis::{self as y, IYieldBasis, YieldBasisMarket},
};
use crate::{
    core::types::{LadderPoint, PoolRef, PoolState},
    gateway::errors::GatewayResult,
    venues::bsc_dex::{ctx::Ctx, node::call, tokens::Tokens},
};
use alloy_primitives::{Address, Bytes, U256};
use serde_json::Value;
fn quote(venue: Address, side: usize, qty: U256) -> (Address, Bytes) {
    data(
        venue,
        IYieldBasis::get_dyCall {
            i: U256::from(side),
            j: U256::from(1 - side),
            qty,
        },
    )
}
fn side(stable: Address, pair: [Address; 2], input: Address) -> Option<usize> {
    pair.contains(&input)
        .then_some(usize::from(input != stable))
}
pub(crate) fn request(
    ctx: &Ctx,
    e: &Entry,
    input: Address,
    qty: U256,
) -> GatewayResult<(Address, Bytes)> {
    let PoolRef::Singleton { id, .. } = e.pool.reference else {
        return Err(y::bad(ctx));
    };
    let (_, _, venue) = y::key(id).ok_or_else(|| y::bad(ctx))?;
    let stable = match &e.pool.state {
        PoolState::YieldBasisLev(s) => s.stable,
        PoolState::OraclePmm(s) => s.pool_token0.ok_or_else(|| y::bad(ctx))?,
        _ => return Err(y::bad(ctx)),
    };
    Ok(quote(
        venue,
        side(stable, [e.pool.token0, e.pool.token1], input).ok_or_else(|| y::bad(ctx))?,
        qty,
    ))
}
pub(crate) async fn official(
    ctx: &Ctx,
    e: &Entry,
    input: Address,
    qty: U256,
    at: &Value,
) -> GatewayResult<U256> {
    let m = bound(
        ctx,
        e.family,
        &e.pool.reference,
        (e.pool.token0, e.pool.token1),
        at,
    )
    .await?;
    let i = side(m.stable, m.coins(), input).ok_or_else(|| y::bad(ctx))?;
    let r = views(ctx, &[quote(m.venue, i, qty)], at).await?;
    call::decode::<IYieldBasis::get_dyCall>(&r[0])
}
async fn bound(
    ctx: &Ctx,
    f: &Family,
    r: &PoolRef,
    pair: (Address, Address),
    at: &Value,
) -> GatewayResult<YieldBasisMarket> {
    let PoolRef::Singleton { manager, id } = r else {
        return Err(y::bad(ctx));
    };
    let (idx, role, venue) = y::key(*id).ok_or_else(|| y::bad(ctx))?;
    let m = y::market(ctx, f, idx, role, at).await?;
    if m.venue != venue
        || m.cryptopool != *manager
        || m.coins() != [pair.0, pair.1] && m.coins() != [pair.1, pair.0]
    {
        return Err(y::bad(ctx));
    }
    Ok(m)
}
async fn levels(
    ctx: &Ctx,
    m: YieldBasisMarket,
    input: Address,
    sizes: &[U256],
    at: &Value,
) -> GatewayResult<Vec<LadderPoint>> {
    let i = side(m.stable, m.coins(), input).ok_or_else(|| y::bad(ctx))?;
    let calls = sizes
        .iter()
        .map(|&qty| quote(m.venue, i, qty))
        .collect::<Vec<_>>();
    let raw = super::balancer_v1::individual(ctx, &calls, at).await?;
    let mut out = Vec::new();
    for (&amount_in, r) in sizes.iter().zip(raw) {
        if let Ok(bytes) = r {
            let amount_out = call::decode::<IYieldBasis::get_dyCall>(&bytes)?;
            if !amount_out.is_zero() {
                out.push(LadderPoint {
                    amount_in,
                    amount_out,
                });
            }
        }
    }
    Ok(out)
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
    let m = bound(ctx, f, r, pair, at).await?;
    if m.role == 1 {
        return Ok(PoolState::YieldBasisLev(Box::new(
            y::read(ctx, m, at).await?,
        )));
    }
    let coin = if counter == pair.0 {
        pair.1
    } else if counter == pair.1 {
        pair.0
    } else {
        return Err(y::bad(ctx));
    };
    let (buys, mut sells) = tokio::try_join!(
        ladder_counter::amounts(ctx, tokens, reg, f.id, counter),
        ladder_counter::amounts(ctx, tokens, reg, f.id, coin)
    )?;
    let buy = levels(ctx, m, counter, &buys, at).await?;
    sells.extend(buy.iter().map(|p| p.amount_out));
    sells.sort_unstable();
    sells.dedup();
    let sell = levels(ctx, m, coin, &sells, at).await?;
    Ok(load_metric::state(
        pair,
        coin,
        m.stable,
        (!buy.is_empty() || !sell.is_empty()).then_some(0),
        buy,
        sell,
    ))
}

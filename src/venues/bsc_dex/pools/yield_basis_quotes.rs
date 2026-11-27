//! Independent original get_dy quotes. Native original execution always requires funded verification.
use super::{
    balancer_v1::{data, views},
    families::Family,
    ladder_counter,
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
    let (_, stable) = shape(e).ok_or_else(|| y::bad(ctx))?;
    Ok(quote(
        venue,
        side(stable, [e.pool.token0, e.pool.token1], input).ok_or_else(|| y::bad(ctx))?,
        qty,
    ))
}
pub(crate) fn shape(e: &Entry) -> Option<(alloy_primitives::B256, Address)> {
    if e.family.variant != super::family_variant::Variant::YieldBasis
        || e.pool.kind != crate::core::types::PoolKind::V2
    {
        return None;
    }
    physical(
        &e.pool.reference,
        [e.pool.token0, e.pool.token1],
        &e.pool.state,
    )
}
fn physical(
    reference: &PoolRef,
    pair: [Address; 2],
    state: &PoolState,
) -> Option<(alloy_primitives::B256, Address)> {
    let PoolRef::Singleton { manager, id } = *reference else {
        return None;
    };
    let (idx, role, venue) = y::key(id)?;
    if manager.is_zero() || (role == 0) != (manager == venue) {
        return None;
    }
    let coins = match state {
        PoolState::YieldBasisLev(s)
            if idx == s.index && role == 1 && venue == s.amm && manager == s.lp =>
        {
            [s.stable, s.lp]
        }
        PoolState::YieldBasisPool(s)
            if id == s.id
                && venue == s.venue
                && manager == s.cryptopool
                && !s.amm.is_zero()
                && s.amm != manager
                && ((role == 0 && venue == manager)
                    || (role == 2 && venue != manager && venue != s.amm)) =>
        {
            s.coins
        }
        _ => return None,
    };
    (!coins[0].is_zero()
        && !coins[1].is_zero()
        && coins[0] != coins[1]
        && (pair == coins || pair == [coins[1], coins[0]]))
    .then_some((id, coins[0]))
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
pub(crate) async fn bound(
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
    let levels = if counter == m.stable {
        [buy, sell]
    } else {
        [sell, buy]
    };
    Ok(PoolState::YieldBasisPool(Box::new(
        crate::core::types::yield_basis::Preview {
            id: m.id(),
            venue: m.venue,
            cryptopool: m.cryptopool,
            amm: m.amm,
            coins: m.coins(),
            levels,
        },
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::B256;
    use std::io::Read;
    #[test]
    fn signed_step_and_cached_state_share_the_same_physical_market() {
        let mut data = String::new();
        flate2::read::GzDecoder::new(
            include_bytes!(
                "../../../../tests/data/first_run_dex_2026-11-26/yield_basis_ethereum.json.gz"
            )
            .as_slice(),
        )
        .read_to_string(&mut data)
        .unwrap();
        let json: Value = serde_json::from_str(&data).unwrap();
        let rows = json["markets"].as_array().unwrap();
        let address = |v: &Value| v.as_str().unwrap().parse::<Address>().unwrap();
        let stable = address(&json["stable"]);
        let mut checked = 0;
        for (index, r) in rows.iter().enumerate() {
            let manager = address(&r["cryptopool"]);
            let amm = address(&r["amm"]);
            for (role, label) in [(0, "cryptopool"), (2, "virtual_pool")] {
                let venue = address(&r[label]);
                if venue.is_zero() {
                    continue;
                }
                let pair = [stable, address(&r["asset"])];
                let mut bytes = [0u8; 32];
                bytes[3..11].copy_from_slice(&(index as u64).to_be_bytes());
                bytes[11] = role;
                bytes[12..].copy_from_slice(venue.as_slice());
                let id = B256::from(bytes);
                let s =
                    PoolState::YieldBasisPool(Box::new(crate::core::types::yield_basis::Preview {
                        id,
                        venue,
                        cryptopool: manager,
                        amm,
                        coins: pair,
                        levels: Default::default(),
                    }));
                let reference = PoolRef::Singleton { manager, id };
                assert_eq!(physical(&reference, pair, &s), Some((id, stable)));
                assert_eq!(
                    physical(&reference, [pair[1], pair[0]], &s),
                    Some((id, stable))
                );
                assert!(physical(&reference, [stable, stable], &s).is_none());
                bytes[10] ^= 1;
                assert!(
                    physical(
                        &PoolRef::Singleton {
                            manager,
                            id: B256::from(bytes)
                        },
                        pair,
                        &s
                    )
                    .is_none()
                );
                assert!(physical(&PoolRef::Singleton { manager: amm, id }, pair, &s).is_none());
                checked += 1;
            }
        }
        assert_eq!(checked, 22);
    }
}

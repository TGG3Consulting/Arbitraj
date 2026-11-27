//! Price and measure the complete Fluid swapHop against a single canonical state.
use super::{
    direct,
    fluid_hop_pricing::sides,
    path::{self, Path},
    route_calc::Leg,
    v2_wallet_sim as sim,
    via::Via,
    white_state::Head,
};
use crate::{
    core::types::Side,
    gateway::{errors::GatewayResult, quote_stage},
    venues::bsc_dex::{
        exec::fluid_lite_plan,
        math::swap_cl::Swapped,
        num,
        pools::{dagrow, fluid_dex_lite as lite},
        wallet::signer,
    },
};
use alloy_primitives::{Address, U256};
use rust_decimal::Decimal;
use std::sync::Arc;
pub(super) async fn paid(
    v: &Via<'_>,
    legs: &[Leg],
    qty: Decimal,
    h: Head,
    selected: (Address, Option<U256>),
) -> GatewayResult<(Path, (u64, usize), U256)> {
    let ctx = &v.q.ctx;
    let (target, fixed) = selected;
    let (input, output) = sides(v);
    let first = legs.first().ok_or_else(|| lite::bad(ctx))?;
    let entries: Vec<_> = legs.iter().map(|l| &l.entry).collect();
    let path = std::iter::once(input)
        .chain(legs.iter().map(|l| l.token_out))
        .collect();
    let at = h.selector();
    let (wire, refreshed) =
        if crate::venues::bsc_dex::exec::motoswap_plan::shape(&first.entry).is_some() {
            let (wire, states) = crate::venues::bsc_dex::exec::motoswap_plan::path_wire(
                ctx, &entries, target, path, &at,
            )
            .await?;
            let mut updated = legs.to_vec();
            for (leg, state) in updated.iter_mut().zip(states) {
                leg.entry.pool.state = crate::core::types::PoolState::Motoswap(Box::new(state));
            }
            super::motoswap_path::fees(&mut updated).ok_or_else(|| lite::bad(ctx))?;
            (wire, Some(updated))
        } else {
            let (t, route) = fluid_lite_plan::hop_terms(ctx, &entries, target, path, &at).await?;
            (sim::Wire::FluidHop(t, Arc::new(route)), None)
        };
    let legs = refreshed.as_deref().unwrap_or(legs);
    let payer = signer::address(ctx)?;
    let wanted = direct::ceil_units(qty, v.coin_dec).ok_or_else(|| lite::bad(ctx))?;
    let buying = v.side == Side::Buy;
    let minimum = if buying { wanted } else { U256::ONE };
    let principal = if !buying {
        if fixed.is_some_and(|x| x != wanted) {
            return Err(lite::bad(ctx));
        }
        wanted
    } else {
        let cash = dagrow::balance(ctx, input, payer, &at).await?;
        let max = quote_stage::buy_budget(ctx.place.id, input)
            .map(|n| num::to_units(n, v.usdt_dec).ok_or_else(|| lite::bad(ctx)))
            .transpose()?
            .unwrap_or(cash)
            .min(cash);
        let amount = match fixed {
            Some(a) => a,
            None if matches!(&wire, sim::Wire::MotoswapPath(..)) => {
                let p = super::route_calc::eval(
                    ctx,
                    legs,
                    v.side,
                    qty,
                    super::pool_tax::coin_keep(legs, v.side, v.taxes),
                )?;
                p.hops
                    .first()
                    .ok_or_else(|| lite::bad(ctx))?
                    .swapped
                    .amount_in
                    .min(max)
            }
            None => {
                let sim::Wire::FluidHop(t, route) = &wire else {
                    return Err(lite::bad(ctx));
                };
                let resolver = lite::shape(&first.entry)
                    .ok_or_else(|| lite::bad(ctx))?
                    .resolver;
                let physical = route.physical(t.wrapped).ok_or_else(|| lite::bad(ctx))?;
                let quote = |amount, exact_in| {
                    lite::hop_quote(
                        ctx,
                        resolver,
                        physical.clone(),
                        &route.markets,
                        amount,
                        exact_in,
                        &at,
                    )
                };
                let guess = quote(wanted, false).await?.min(max);
                if guess.is_zero() {
                    return Err(lite::bad(ctx));
                }
                if quote(guess, true).await? >= wanted {
                    guess
                } else {
                    // Fluid normalizes each side to nine decimals. Find a paid principal on a higher input quantum.
                    let quantum = U256::from(10)
                        .checked_pow(U256::from(first.dec_in.saturating_sub(9)))
                        .ok_or_else(|| lite::bad(ctx))?;
                    let mut lower = guess;
                    let mut gap = quantum;
                    let mut upper;
                    loop {
                        upper = guess.saturating_add(gap).min(max);
                        if upper <= lower {
                            return Err(lite::bad(ctx));
                        }
                        if quote(upper, true).await? >= wanted {
                            break;
                        }
                        lower = upper;
                        gap = gap.saturating_mul(U256::from(2));
                    }
                    while upper - lower > U256::ONE {
                        let mid = lower + (upper - lower) / U256::from(2);
                        if quote(mid, true).await? >= wanted {
                            upper = mid;
                        } else {
                            lower = mid;
                        }
                    }
                    upper
                }
            }
        };
        if amount > max || amount.is_zero() {
            return Err(lite::bad(ctx));
        }
        amount
    };
    let order = sim::Order {
        payer,
        target,
        input,
        output,
        amount: principal,
        minimum,
        wire,
    };
    let fill = sim::measure(ctx, h, order, true).await?;
    let flows = fill
        .fluid_legs
        .filter(|f| f.len() == legs.len())
        .ok_or_else(|| lite::bad(ctx))?;
    let mut hops = Vec::with_capacity(legs.len());
    for (i, (leg, (amount_in, amount_out))) in legs.iter().zip(flows).enumerate() {
        let mut u = leg.usable.clone();
        u.gas = if i == 0 { fill.swap_gas } else { 0 };
        let swapped = Swapped {
            amount_in,
            amount_out,
            complete: true,
        };
        hops.push(path::hop(
            ctx,
            leg.entry.clone(),
            u,
            swapped,
            (leg.token_in, leg.token_out),
            (leg.dec_in, leg.dec_out),
        )?);
    }
    let cash = if buying { fill.charged } else { fill.received };
    let usdt = num::to_decimal(cash, v.usdt_dec).ok_or_else(|| lite::bad(ctx))?;
    Ok((
        Path {
            hops,
            coins: qty,
            usdt,
        },
        fill.approval,
        principal,
    ))
}

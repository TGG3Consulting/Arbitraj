//! Puzzle Ranges charges the vault fee before pricing and leaves one million scaled units in the pool.
use super::{
    balancer_math::{div_down, div_up, mul_down, mul_up, one},
    balancer_v3_weighted::{complement, pow_up},
};
use alloy_primitives::U256;

pub(super) fn on_swap(
    virtual_balances: &[U256],
    real_balances: &[U256],
    weights: &[U256],
    i: usize,
    j: usize,
    amount: U256,
    exact_in: bool,
) -> Option<U256> {
    let vin = *virtual_balances.get(i)?;
    let vout = *virtual_balances.get(j)?;
    let capacity = real_balances
        .get(j)?
        .checked_sub(U256::from(1_000_000u32))?;
    let (wi, wo) = (*weights.get(i)?, *weights.get(j)?);
    if exact_in {
        let base = div_up(vin, vin.checked_add(amount)?)?;
        let power = pow_up(base, div_down(wi, wo)?)?;
        Some(mul_down(vout, complement(power))?.min(capacity))
    } else {
        if amount > capacity {
            return None;
        }
        let base = div_up(vout, vout.checked_sub(amount)?)?;
        let power = pow_up(base, div_up(wo, wi)?)?;
        mul_up(vin, power.checked_sub(one())?)
    }
}

use super::{balancer_math, full_math};
use crate::core::types::pool_vault::{VaultPoolData, VaultV3Rule};

pub(crate) fn active(d: &VaultPoolData) -> bool {
    d.v3.as_ref()
        .is_some_and(|v| matches!(v.rule, VaultV3Rule::PuzzleRanges { .. }))
}

/// The largest whole raw payout that leaves the protocol's scaled reserve in place.
fn capacity(d: &VaultPoolData, j: usize) -> Option<U256> {
    let v = d.v3.as_ref()?;
    let cap = v.live.get(j)?.checked_sub(U256::from(1_000_000u32))?;
    if cap < v.min_trade {
        return None;
    }
    let scale = d.scaling.get(j)?.checked_mul(*v.rates.get(j)?)?;
    Some(full_math::mul_div(cap, one(), scale)?.min(*d.balances.get(j)?))
}

/// Obtain a spot reference above the vault minimum on both assets, even for a shallow cash reserve.
pub(crate) fn probe(d: &VaultPoolData, i: usize, j: usize, usual: U256) -> Option<(U256, U256)> {
    let v = d.v3.as_ref()?;
    if !matches!(v.rule, VaultV3Rule::PuzzleRanges { enabled: true, .. })
        || capacity(d, j)?.is_zero()
    {
        return None;
    }
    let min_raw = |k: usize| {
        full_math::mul_div_up(
            v.min_trade,
            one(),
            d.scaling.get(k)?.checked_mul(*v.rates.get(k)?)?,
        )
    };
    let mut n = usual.max(min_raw(i)?);
    if let Some(out) = balancer_math::amount_out(d, i, j, n).filter(|x| !x.is_zero()) {
        return Some((n, out));
    }
    let target = min_raw(j)?;
    if let Some(needed) = balancer_math::amount_in(d, i, j, target) {
        n = n.max(needed)
    }
    // The fee or final raw conversion can put the inverse one unit below the first executable offer.
    for budget in [n, n.checked_add(U256::ONE)?] {
        if let Some(out) = balancer_math::amount_out(d, i, j, budget).filter(|x| !x.is_zero()) {
            return Some((budget, out));
        }
    }
    None
}

/// Bound a price-band search by the wallet's maximum payout and the already measured reference price.
/// A weighted-pool 30% cap does not apply to Puzzle Ranges.
pub(crate) fn depth_cap(
    d: &VaultPoolData,
    i: usize,
    j: usize,
    probe: U256,
    reference: U256,
    limit: U256,
    buy: bool,
) -> Option<U256> {
    let highest = capacity(d, j)?;
    let basis = U256::from(10_000);
    let at_spot = full_math::mul_div_up(highest, probe, reference)?;
    let price_bound = if buy {
        full_math::mul_div_up(at_spot, limit, basis)
    } else {
        full_math::mul_div_up(at_spot, basis, limit)
    }?;
    // Once maximum physical proceeds are reached, a larger debit can only reduce profit.
    let boundary = balancer_math::amount_in(d, i, j, highest)
        .filter(|x| balancer_math::amount_out(d, i, j, *x).is_some_and(|out| out >= highest));
    Some(
        boundary
            .map_or(price_bound, |x| x.min(price_bound))
            .max(probe),
    )
}

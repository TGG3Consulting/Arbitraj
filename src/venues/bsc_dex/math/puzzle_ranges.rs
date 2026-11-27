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
    let scale = d
        .scaling
        .get(j)?
        .checked_mul(super::balancer_v3::rate_up(*v.rates.get(j)?))?;
    Some(full_math::mul_div(cap, one(), scale)?.min(*d.balances.get(j)?))
}

/// Largest fixed-point base whose protocol power can pay the required number of output units.
/// Search the 18-decimal base, not raw token amounts. Equal weights invert exactly in constant time.
fn maximum_base(power_limit: U256, wi: U256, wo: U256) -> Option<U256> {
    let unit = one();
    let exponent = div_down(wi, wo)?;
    if exponent == unit {
        return Some(power_limit);
    }
    // An out-of-domain negative logarithm is below the representable curve. The eventual
    // boundary must itself price successfully; forward verification below rejects an unreachable target.
    let below = |base| pow_up(base, exponent).is_none_or(|p| p <= power_limit);
    let guess = pow_up(power_limit, div_down(wo, wi)?)
        .unwrap_or(U256::ZERO)
        .min(unit);
    let (mut lo, mut hi) = (U256::ZERO, unit);
    let mut delta = U256::ONE;
    if below(guess) {
        lo = guess;
        while lo < unit {
            hi = guess.saturating_add(delta).min(unit);
            if hi == unit || !below(hi) {
                break;
            }
            lo = hi;
            delta *= U256::from(2);
        }
    } else {
        hi = guess;
        while !hi.is_zero() {
            lo = guess.saturating_sub(delta);
            if lo.is_zero() || below(lo) {
                break;
            }
            hi = lo;
            delta *= U256::from(2);
        }
    }
    while hi - lo > U256::ONE {
        let mid = lo + (hi - lo) / U256::from(2);
        if below(mid) {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    pow_up(lo, exponent)
        .filter(|p| *p <= power_limit)
        .map(|_| lo)
}

/// Smallest raw EXACT_IN debit for a requested wallet payout. The executor uses EXACT_IN
/// even when the trading engine asks how much it costs to buy a given number of tokens.
pub(super) fn first_debit(
    d: &VaultPoolData,
    i: usize,
    j: usize,
    requested: U256,
) -> Option<(U256, U256)> {
    use crate::core::types::pool_vault::VaultCurve;
    let v = d.v3.as_ref()?;
    let VaultV3Rule::PuzzleRanges {
        virtual_balances,
        enabled: true,
        ..
    } = &v.rule
    else {
        return None;
    };
    let VaultCurve::Weighted { weights } = &d.curve else {
        return None;
    };
    if d.paused
        || i == j
        || requested.is_zero()
        || requested > capacity(d, j)?
        || v.via.get(i).is_some_and(Option::is_some)
        || v.via.get(j).is_some_and(Option::is_some)
    {
        return None;
    }
    let den_out = d
        .scaling
        .get(j)?
        .checked_mul(super::balancer_v3::rate_up(*v.rates.get(j)?))?;
    let needed = full_math::mul_div_up(requested, den_out, one())?.max(v.min_trade);
    let complement = full_math::mul_div_up(needed, one(), *virtual_balances.get(j)?)?;
    let base = maximum_base(
        one().checked_sub(complement)?,
        *weights.get(i)?,
        *weights.get(j)?,
    )?;
    let vin = *virtual_balances.get(i)?;
    // ceil(vin / (vin + net)) <= base, with the same fixed-point scale as on_swap.
    let net = full_math::mul_div_up(vin, one(), base)?
        .checked_sub(vin)?
        .max(v.min_trade);
    let gross = full_math::mul_div_up(net, one(), one().checked_sub(d.fee)?)?;
    let den_in = d.scaling.get(i)?.checked_mul(*v.rates.get(i)?)?;
    let debit = full_math::mul_div_up(gross, one(), den_in)?;
    let credit = balancer_math::amount_out(d, i, j, debit)?;
    (credit >= requested).then_some((debit, credit))
}

/// Obtain a spot reference above the vault minimum on both assets.
pub(crate) fn probe(d: &VaultPoolData, i: usize, j: usize, usual: U256) -> Option<(U256, U256)> {
    if let Some(credit) = balancer_math::amount_out(d, i, j, usual).filter(|x| !x.is_zero()) {
        return Some((usual, credit));
    }
    first_debit(d, i, j, U256::ONE)
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
    let boundary = first_debit(d, i, j, highest).map(|(debit, _)| debit);
    Some(
        boundary
            .map_or(price_bound, |x| x.min(price_bound))
            .max(probe),
    )
}

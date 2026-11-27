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

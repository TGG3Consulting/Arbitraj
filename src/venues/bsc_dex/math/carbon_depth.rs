//! Marginal-rate bounds and depth across all funded orders of a Carbon pair.
use super::{carbon, carbon_match::capacity, full_math::mul_div};
use crate::core::types::{CarbonOrder, CarbonState};
use alloy_primitives::{Address, U256};

const Q: u64 = 1 << 48;
fn marginal(o: &CarbonOrder, ceil: bool) -> Option<U256> {
    if o.y == 0 || o.z < o.y {
        return None;
    }
    let a = carbon::expand(o.a)?;
    let b = carbon::expand(o.b)?;
    let n = U256::from(o.y) * a;
    let z = U256::from(o.z);
    Some((if ceil { n.div_ceil(z) } else { n / z }) + b)
}

pub fn best(s: &CarbonState, input: Address) -> Option<U256> {
    let i = s.tokens.iter().position(|x| *x == input)?;
    s.strategies
        .iter()
        .filter_map(|st| marginal(&st.orders[1 - i], true))
        .max()
        .filter(|rate| !rate.is_zero())
}

pub fn coins(s: &CarbonState, input: Address, amount: U256) -> Option<U256> {
    let rate = best(s, input)?;
    let one = U256::from(Q);
    mul_div(amount, one * one, rate * rate)
}

pub fn band(s: &CarbonState, input: Address, buy: bool, width: u64) -> Option<U256> {
    let i = s.tokens.iter().position(|x| *x == input)?;
    let Some(top) = best(s, input) else {
        return Some(U256::ZERO);
    };
    let scale = U256::from(1_000_000_000_000_000_000u64);
    let w = U256::from(width);
    let cutoff = (if buy {
        mul_div(top * top, scale, scale + w)?
    } else {
        mul_div(top * top, scale.checked_sub(w)?, scale)?
    })
    .root(2);
    let mut total = U256::ZERO;
    for st in &s.strategies {
        let o = &st.orders[1 - i];
        if o.y == 0 || marginal(o, true).is_none_or(|p| p < cutoff) {
            continue;
        }
        let a = carbon::expand(o.a)?;
        let b = carbon::expand(o.b)?;
        let mut target = o.y;
        if cutoff > b {
            if a.is_zero() {
                continue;
            }
            let leave = (U256::from(o.z) * (cutoff - b)).div_ceil(a);
            target = U256::from(o.y).saturating_sub(leave).to::<u128>();
        }
        if let Some((n, pay)) = capacity(o, u128::MAX - st.orders[i].y, true, target) {
            total = total.checked_add(U256::from(if buy { pay } else { n }))?;
        }
    }
    Some(total)
}

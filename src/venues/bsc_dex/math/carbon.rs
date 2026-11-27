//! CarbonController v6: math for a single trade action. The Solidity implementation
//! scales products before dividing; using an unrounded rational function changes payouts.
use super::full_math::{mul_div, mul_div_up};
use crate::core::types::CarbonOrder;
#[cfg(test)]
#[path = "carbon_tests.rs"]
mod tests;
use alloy_primitives::U256;

const ONE: u64 = 1 << 48;
const PPM: u32 = 1_000_000;

fn expand(rate: u64) -> Option<U256> {
    let exp = rate / ONE;
    (exp < 48).then(|| U256::from(rate % ONE) << exp as usize)
}

// MathEx.minFactor uses the 512-bit product. Its result is at least one.
fn factor(x: U256, y: U256) -> Option<U256> {
    Some(mul_div_up(x, y, U256::MAX)?.max(U256::ONE))
}

pub fn amount(o: &CarbonOrder, by_target: bool, size: u128) -> Option<u128> {
    if size == 0 || o.y == 0 || o.z < o.y || (by_target && size > o.y) {
        return None;
    }
    let x = U256::from(size);
    let y = U256::from(o.y);
    let z = U256::from(o.z);
    let a = expand(o.a)?;
    let b = expand(o.b)?;
    let one = U256::from(ONE);
    let result = if a.is_zero() {
        if b.is_zero() {
            return None;
        }
        if by_target {
            mul_div_up(x, one * one, b * b)?
        } else {
            mul_div(x, b * b, one * one)?
        }
    } else {
        let z1 = z * one;
        let rate = y * a + z * b;
        if by_target {
            let end = rate.checked_sub(x * a)?;
            let f = factor(z1, z1)?.max(factor(rate, end)?);
            mul_div_up(x, mul_div_up(z1, z1, f)?, mul_div(rate, end, f)?)?
        } else {
            let rx = rate.checked_mul(x)?;
            let f = factor(z1, z1)?.max(factor(rx, a)?);
            let left = mul_div_up(z1, z1, f)?;
            let right = mul_div_up(rx, a, f)?;
            if let Some(den) = left.checked_add(right) {
                mul_div(rate, rx / f, den)?
            } else {
                rate.checked_div(a.checked_add(mul_div_up(z1, z1, rx)?)?)?
            }
        }
    };
    let n = u128::try_from(result).ok()?;
    if n == 0 || (!by_target && n > o.y) {
        return None;
    }
    Some(n)
}

/// Applies the pair fee once, after summing all legs of a controller transaction.
pub fn with_fee(total: u128, by_target: bool, ppm: u32) -> Option<u128> {
    let keep = U256::from(PPM.checked_sub(ppm)?);
    let rate = U256::from(PPM);
    let n = U256::from(total);
    let out = if by_target {
        mul_div_up(n, rate, keep)?
    } else {
        mul_div(n, keep, rate)?
    };
    u128::try_from(out).ok()
}

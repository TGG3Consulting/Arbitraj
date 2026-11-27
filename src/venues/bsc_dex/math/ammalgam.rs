//! Exact integer branches of AmmalgamPair.calculateBalanceAfterFees and QuadraticSwapFees.
//! Source of the beacon implementation on Ethereum at 26494988:
//! https://repo.sourcify.dev/1/0x5C0d819609Eee2cdCf326f0e898A8447e06a45Be
use super::{full_math, swap_cl::Swapped};
use crate::core::types::AmmalgamState;
use alloy_primitives::U256;

const Q64: u128 = 1 << 64;
const MIN_FEE: u128 = 0x1999999999999999;
const BIPS: u128 = 10_000;
const MAX_QUADRATIC_BIPS: u128 = 4_000;
const BUFFER_NUMERATOR: u8 = 20;

fn fee_bips(amount: U256, reserve: U256, reference: U256) -> Option<U256> {
    if reference.is_zero() || amount.is_zero() {
        return None;
    }
    let base = reserve.max(reference);
    let past = amount.checked_add(reserve)?.saturating_sub(base);
    let deviation = past.checked_add((base - reference).checked_mul(U256::from(2))?)?;
    let slope = U256::from(MAX_QUADRATIC_BIPS * Q64);
    let fee = if deviation <= reference {
        slope.checked_mul(deviation)? / reference
    } else {
        let tail = reference.checked_mul(U256::from(Q64))? / deviation;
        U256::from(MAX_QUADRATIC_BIPS).checked_mul(U256::from(2 * Q64) - tail)?
    };
    Some((fee.checked_mul(past)? / amount).max(U256::from(MIN_FEE)))
}

// Convert.calculateReserveAdjustmentsForMissingAssets is the lesser of the two branches.
fn adjusted(reserve: U256, missing: U256) -> Option<U256> {
    Some(
        reserve.min(
            reserve
                .checked_sub(missing)?
                .checked_mul(U256::from(BUFFER_NUMERATOR))?,
        ),
    )
}

impl AmmalgamState {
    fn payout(&self, i: usize, amount: U256) -> Option<U256> {
        let o = 1 - i;
        let r = self.reserves;
        if amount.is_zero() {
            return Some(U256::ZERO);
        }
        let fee = amount
            .checked_mul(fee_bips(amount, r[i], self.reference[i])?)?
            .div_ceil(U256::from(BIPS * Q64));
        let after_in = adjusted(r[i].checked_add(amount.checked_sub(fee)?)?, self.missing[i])?;
        if after_in.is_zero() {
            return None;
        }
        let invariant =
            adjusted(r[i], self.missing[i])?.checked_mul(adjusted(r[o], self.missing[o])?)?;
        let scaled = invariant.div_ceil(after_in);
        let remaining = Self::reserve_for(scaled, self.missing[o])?;
        r[o].checked_sub(remaining)
    }

    /// A reserve must satisfy both straight branches of the adjusted-reserve function.
    fn reserve_for(adjusted: U256, missing: U256) -> Option<U256> {
        Some(adjusted.max(missing.checked_add(adjusted.div_ceil(U256::from(BUFFER_NUMERATOR)))?))
    }
    fn input_for(&self, i: usize, wanted: U256, max: U256) -> Option<U256> {
        let o = 1 - i;
        let cash_out = self.reserves[o].checked_sub(self.missing[o])?;
        if wanted >= cash_out || wanted > self.cash[o] {
            return None;
        }
        let right = adjusted(self.reserves[o] - wanted, self.missing[o])?;
        if right.is_zero() {
            return None;
        }
        let k = adjusted(self.reserves[i], self.missing[i])?
            .checked_mul(adjusted(self.reserves[o], self.missing[o])?)?;
        let net =
            Self::reserve_for(k.div_ceil(right), self.missing[i])?.checked_sub(self.reserves[i])?;
        if net.is_zero() {
            return None;
        }
        let unit = U256::from(BIPS * Q64);
        let numerator = net.checked_mul(unit)?;
        let mut gross = net;
        // fee_bips never decreases. At this candidate the minimum gross covering net at its
        // current rate is a lower bound for every feasible input. Jump directly to that bound.
        // Equal bounds are an executable fixed point, including integer rounding on the fee.
        while gross <= max {
            let rate = fee_bips(gross, self.reserves[i], self.reference[i])?;
            let next = numerator.div_ceil(unit.checked_sub(rate)?);
            if next <= gross {
                return Some(gross);
            }
            gross = next;
        }
        None
    }

    pub(crate) fn swap(&self, zero_in: bool, exact_in: bool, amount: U256) -> Option<Swapped> {
        let i = usize::from(!zero_in);
        if amount.is_zero() || self.reserves[i] <= self.missing[i] {
            return None;
        }
        // The original pair stores its updated reserves in uint112.
        let max = (U256::from(u128::MAX >> 16)).checked_sub(self.reserves[i])?;
        let input = if exact_in {
            amount
        } else {
            self.input_for(i, amount, max)?
        };
        if input > max {
            return None;
        }
        // Like the pair's swap, the protected wallet limits its request to actual
        // output inventory and leaves at least one raw unit above uncovered borrows.
        let pair_limit = self.reserves[1 - i]
            .checked_sub(self.missing[1 - i])?
            .checked_sub(U256::ONE)?;
        let gross = self.payout(i, input)?.min(pair_limit).min(self.cash[1 - i]);
        if gross.is_zero() || (!exact_in && gross < amount) {
            return None;
        }
        Some(Swapped {
            amount_in: input,
            amount_out: if exact_in { gross } else { amount },
            complete: true,
        })
    }
}

#[path = "ammalgam_depth.rs"]
mod depth;

#[cfg(test)]
#[path = "ammalgam_tests.rs"]
mod tests;

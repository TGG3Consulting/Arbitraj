//! ToonSwap Pair.sol. First price against the reserves, then move both fees and record the actual balances.
use super::{full_math::mul_div_up, swap_cl::Swapped, v2};
use crate::core::types::ToonState;
use alloy_primitives::U256;

impl ToonState {
    pub fn trade(&self, zero_for_one: bool, exact_in: bool, size: U256) -> Option<(Swapped, Self)> {
        if size.is_zero() || self.reserves.contains(&U256::ZERO) {
            return None;
        }
        let i = usize::from(!zero_for_one);
        let j = 1 - i;
        let [rin, rout] = [self.reserves[i], self.reserves[j]];
        let unit = U256::from(10_000);
        let fee = U256::from(25);
        let debit = if exact_in {
            size
        } else {
            let required = mul_div_up(
                rin.checked_mul(unit)?,
                size,
                rout.checked_sub(size)?.checked_mul(unit - fee)?,
            )?;
            rin.checked_add(required)?
                .saturating_sub(self.balances[i])
                .max(U256::ONE)
        };
        let mut available = self.balances;
        available[i] = available[i].checked_add(debit)?;
        let effective = available[i].checked_sub(rin)?;
        let output = v2::amount_out(effective, rin, rout, (unit - fee, unit))?;
        if output.is_zero() || output >= rout || (!exact_in && output < size) {
            return None;
        }
        available[j] = available[j].checked_sub(output)?;
        let mut adjusted = [U256::ZERO; 2];
        let mut next = self.clone();
        for k in 0..2 {
            let reserve_after_output = self.reserves[k] - if k == j { output } else { U256::ZERO };
            let arrived = available[k].saturating_sub(reserve_after_output);
            adjusted[k] = available[k]
                .checked_mul(unit)?
                .checked_sub(arrived.checked_mul(fee)?)?;
            let pool_fee = arrived.wrapping_mul(fee) / unit;
            let mut balance = available[k];
            for (&rate, &recipient) in self.rates.iter().zip(self.recipients.iter()) {
                // The deployed Pair uses unchecked multiplication here. Each transfer has its own floor.
                let paid = pool_fee.wrapping_mul(rate) / unit;
                if balance < paid {
                    return None;
                }
                if recipient != self.pool {
                    balance -= paid;
                }
            }
            if balance.bit_len() > 112 {
                return None;
            }
            next.balances[k] = balance;
        }
        if adjusted[0].checked_mul(adjusted[1])?
            < rin.checked_mul(rout)?.checked_mul(unit * unit)?
        {
            return None;
        }
        next.reserves = next.balances;
        Some((
            Swapped {
                amount_in: debit,
                amount_out: output,
                complete: true,
            },
            next,
        ))
    }
}

//! Формулы DeFiPlaza.swap и StablePlaza.getOutFromIn. Округление до физической единицы.
use super::swap_cl::Swapped;
use crate::core::types::defiplaza::{DefiPlazaCurve, DefiPlazaState};
use alloy_primitives::{Address, U256};

impl DefiPlazaState {
    fn params(&self) -> (U256, U256, U256) {
        match self.curve {
            DefiPlazaCurve::Multi { one_minus_fee } => (
                U256::from(one_minus_fee),
                U256::from(crate::core::types::defiplaza::MULTI_FEE_SCALE),
                U256::ZERO,
            ),
            DefiPlazaCurve::Stable { fee_bps, delta } => (
                U256::from(10_000u64 - u64::from(fee_bps)),
                U256::from(10_000),
                U256::from(delta),
            ),
        }
    }

    pub fn effective(&self, i: usize) -> Option<U256> {
        self.reserves[i]
            .checked_add(self.params().2)?
            .checked_mul(self.factors[i])
    }

    pub fn after(&self, input: Address, spent: U256, received: U256) -> Option<Self> {
        let i = self.tokens.iter().position(|a| *a == input)?;
        let j = 1 - i;
        let mut next = self.clone();
        next.balances[i] = self.balances[i].checked_add(spent)?;
        next.balances[j] = self.balances[j].checked_sub(received)?;
        next.reserves[i] = next.balances[i].checked_div(self.factors[i])?;
        next.reserves[j] = self.reserves[j].checked_sub(received.div_ceil(self.factors[j]))?;
        Some(next)
    }

    pub fn swap(&self, input: Address, exact_in: bool, amount: U256) -> Option<Swapped> {
        let i = self.tokens.iter().position(|a| *a == input)?;
        let j = 1 - i;
        let (keep, base, delta) = self.params();
        let x = self.reserves[i].checked_add(delta)?;
        let y = self.reserves[j].checked_add(delta)?;
        let d = self.factors[i];
        let e = self.factors[j];
        if amount.is_zero()
            || keep.is_zero()
            || d.is_zero()
            || e.is_zero()
            || x.is_zero()
            || y.is_zero()
        {
            return None;
        }
        let spend = if exact_in {
            amount
        } else {
            let wanted = amount.div_ceil(e);
            if wanted > self.reserves[j] || wanted >= y {
                return None;
            }
            let numerator = x.checked_mul(base)?.checked_mul(wanted)?;
            numerator
                .div_ceil(keep.checked_mul(y - wanted)?)
                .checked_mul(d)?
        };
        let dx = spend.checked_div(d)?.checked_mul(keep)?;
        let gross = dx
            .checked_mul(y)?
            .checked_div(x.checked_mul(base)?.checked_add(dx)?)?;
        let out = gross.min(self.reserves[j]).checked_mul(e)?;
        if out.is_zero() || out > self.balances[j] || (!exact_in && out < amount) {
            return None;
        }
        if matches!(self.curve, DefiPlazaCurve::Stable { .. }) {
            let actual = self.balances[i].checked_add(spend)?.checked_div(d)?;
            let credit = actual.checked_sub(self.reserves[i])?;
            let b0 = actual
                .checked_add(delta)?
                .checked_mul(base)?
                .checked_sub(credit.checked_mul(base - keep)?)?;
            let b1 = y.checked_sub(out.div_ceil(e))?;
            if b0.checked_mul(b1)? < x.checked_mul(y)?.checked_mul(base)? {
                return None;
            }
        }
        Some(Swapped {
            amount_in: spend,
            amount_out: out,
            complete: true,
        })
    }
}

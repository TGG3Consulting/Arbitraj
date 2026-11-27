//! EmpirePair._balanceOfSelf прибавляет долг только к своему sweepableToken.
//! Размер выплаты ограничивают исходная кривая и реальный остаток ERC20.
use super::{swap_cl::Swapped, v2};
use crate::core::types::EmpireState;
use alloy_primitives::U256;

impl EmpireState {
    pub(crate) fn credit(&self, zero_in: bool, amount: U256) -> Option<U256> {
        let i = usize::from(!zero_in);
        let mut total = self.balances[i].checked_add(amount)?;
        if self.debt_aware {
            total = total.checked_add(self.debt[i])?;
        }
        let observed = total.checked_sub(self.reserves[i])?;
        if !self.debt_aware && observed > amount {
            return None;
        }
        Some(observed.min(amount))
    }

    /// Точный вход — фактически пришедшие в пару атомы, до удержания выходного токена.
    pub(crate) fn paid(&self, zero_in: bool, amount: U256) -> Option<(Swapped, Self)> {
        let i = usize::from(!zero_in);
        let o = 1 - i;
        let [rin, rout] = [self.reserves[i], self.reserves[o]];
        let physical = self.balances[i].checked_add(amount)?;
        let logical = physical.checked_add(self.debt[i])?;
        let credit = self.credit(zero_in, amount)?;
        let output = v2::amount_out(credit, rin, rout, (U256::from(997), U256::from(1000)))?;
        if credit.is_zero() || output.is_zero() || output >= rout {
            return None;
        }
        let payout = self.balances[o].checked_sub(output)?;
        let other = payout.checked_add(self.debt[o])?;
        let max = (U256::ONE << 112) - U256::ONE;
        if logical > max || other > max {
            return None;
        }
        let effective_in = logical.saturating_sub(rin);
        let effective_out = other.saturating_sub(rout - output);
        let adjusted = |b: U256, x: U256| {
            b.checked_mul(U256::from(1000))?
                .checked_sub(x.checked_mul(U256::from(3))?)
        };
        if adjusted(logical, effective_in)?.checked_mul(adjusted(other, effective_out)?)?
            < rin.checked_mul(rout)?.checked_mul(U256::from(1_000_000))?
        {
            return None;
        }
        let mut after = self.clone();
        after.balances[i] = physical;
        after.balances[o] = payout;
        after.reserves[i] = logical;
        after.reserves[o] = other;
        Some((
            Swapped {
                amount_in: amount,
                amount_out: output,
                complete: true,
            },
            after,
        ))
    }

    /// Минимальный собственный вход для достигаемого gross. Итоговая платёжная проба учитывает налог ERC20.
    pub(crate) fn quote(&self, zero_in: bool, exact_in: bool, amount: U256) -> Option<Swapped> {
        if exact_in {
            return self.paid(zero_in, amount).map(|r| r.0);
        }
        let i = usize::from(!zero_in);
        let o = 1 - i;
        let [rin, rout] = [self.reserves[i], self.reserves[o]];
        if amount.is_zero() || amount >= rout || amount > self.balances[o] {
            return None;
        }
        let starting = self.balances[i].checked_add(if self.debt_aware {
            self.debt[i]
        } else {
            U256::ZERO
        })?;
        let required = rin
            .checked_mul(amount)?
            .checked_mul(U256::from(1000))?
            .div_ceil((rout - amount).checked_mul(U256::from(997))?);
        let debit = required.checked_add(rin.saturating_sub(starting))?;
        let (fill, _) = self.paid(zero_in, debit)?;
        (fill.amount_out >= amount).then_some(Swapped {
            amount_out: amount,
            ..fill
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn outstanding_and_returned_debt_preserve_physical_limits() {
        let n = U256::from;
        let mut s = EmpireState {
            reserves: [n(1000u64), n(1000)],
            balances: [n(950), n(1000)],
            debt: [n(50), n(0)],
            debt_aware: true,
            exact_tokens: true,
        };
        let (b, next) = s.paid(true, n(10)).unwrap();
        assert_eq!(
            (b.amount_out, next.reserves[0], next.balances[0]),
            (n(9), n(1010), n(960))
        );
        assert_eq!(s.quote(false, false, n(950)).unwrap().amount_out, n(950));
        assert!(s.quote(false, false, n(951)).is_none());
        s.debt_aware = false;
        assert!(s.paid(true, n(10)).is_none());
        assert_eq!(s.paid(true, n(60)).unwrap().0.amount_out, n(9));
        s.balances[0] += n(50);
        s.debt[0] = U256::ZERO;
        assert_eq!(s.paid(true, n(10)).unwrap().0.amount_out, b.amount_out);
        assert!(s.quote(false, false, n(951)).is_some());
    }
}

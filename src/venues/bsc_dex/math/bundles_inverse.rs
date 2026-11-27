//! Обратный расчёт входа. Каждая следующая нижняя граница следует из цены и комиссии при предыдущем объёме.
use super::{
    bundles::{Book, Fill, Market},
    full_math::{div_up, mul_div_up},
    swap_cl::Swapped,
};
use alloy_primitives::{Address, U256};

impl Market {
    fn minimum_principal(&self, i: usize, credit: U256, embedded: bool) -> Option<U256> {
        let unit = U256::from(1000);
        let tier = U256::from(self.tier);
        let desired = if embedded && i == 1 {
            mul_div_up(credit, unit + tier, unit)?
        } else {
            credit
        };
        let net = mul_div_up(
            self.reserves[i],
            desired,
            self.reserves[1 - i].checked_sub(desired)?,
        )?;
        if embedded && i == 0 {
            mul_div_up(net, unit + tier, unit)
        } else {
            Some(net)
        }
    }
}

impl Book {
    pub(super) fn refill_cost(&self, target: U256) -> Option<U256> {
        if target.is_zero() {
            return None;
        }
        let f = &self.fee_pair;
        let raw = target.checked_mul(U256::from(1010))? / U256::from(1000);
        f.reserves[1]
            .checked_mul(raw)?
            .checked_div(f.reserves[0].checked_sub(raw)?)?
            .checked_add(U256::ONE)
    }

    pub fn swap(&self, source: Address, exact_in: bool, amount: U256) -> Option<(Swapped, Fill)> {
        if amount.is_zero() {
            return None;
        }
        let mut debit = amount;
        if !exact_in {
            let m = &self.market;
            let i = m.tokens.iter().position(|&t| t == source)?;
            let embedded = m.tokens[0] == self.bun;
            let unit = U256::from(1000);
            let tier = U256::from(m.tier);
            // Pair requires at least 1000 raw BUN. Include that floor in the inverse.
            // Surplus in either token can also satisfy part of the minimum fee.
            let mut principal = m.minimum_principal(i, amount, embedded)?;
            if embedded && m.balances[0].checked_sub(m.fee_reserve)? == m.reserves[0] {
                let minimum = if i == 0 {
                    mul_div_up(unit, unit + tier, tier)?
                } else {
                    m.minimum_principal(i, mul_div_up(unit, unit, tier)?, true)?
                };
                principal = principal.max(minimum);
            }
            debit = principal;
            if !embedded {
                let (side, rate) = m.oracle?;
                let fee_pair = &self.fee_pair;
                if fee_pair.tokens != [self.bun, self.weth]
                    || fee_pair.tier != 10
                    || !m.tokens.contains(&self.weth)
                {
                    return None;
                }
                // This bound uses only fee units the two contracts actually require.
                let mut min_fee = unit;
                if fee_pair.balances[0].checked_sub(fee_pair.fee_reserve)? == fee_pair.reserves[0] {
                    min_fee = mul_div_up(unit, unit, U256::from(10))?;
                }
                let fee_basis = mul_div_up(min_fee, U256::ONE << 112, rate)?;
                let needed = if side == i {
                    mul_div_up(fee_basis, unit, tier)?
                } else {
                    m.minimum_principal(i, mul_div_up(fee_basis, unit + tier, tier)?, false)?
                };
                principal = principal.max(needed);
                debit = principal;
                loop {
                    let gross = m.price(i, debit, false)?;
                    let fee = m.ceiling_fee(i, debit, gross, false)?;
                    let cost = self.refill_cost(fee)?;
                    let lower = if source == self.weth {
                        // n² ≥ principal·(n+cost). Taking the integer ceiling preserves the least debit.
                        let square = principal.checked_mul(
                            principal.checked_add(cost.checked_mul(U256::from(4))?)?,
                        )?;
                        let root: U256 = square.root(2);
                        let ceil = root + U256::from(u8::from(root * root != square));
                        div_up(principal.checked_add(ceil)?, U256::from(2))?
                    } else {
                        m.minimum_principal(i, amount.checked_add(cost)?, false)?
                            .max(principal)
                    };
                    if lower <= debit {
                        break;
                    }
                    debit = lower;
                }
            }
        }
        let fill = self.exact_in(source, debit, false)?;
        if !exact_in && fill.credit < amount {
            return None;
        }
        Some((
            Swapped {
                amount_in: debit,
                amount_out: fill.credit,
                complete: true,
            },
            fill,
        ))
    }
}

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

    // The router rounds its WETH budget twice. Its nominal quote can be positive before
    // the fee pool receives enough WETH to preserve its invariant. Skip only budgets that
    // cannot fix the shortage, up to the next discontinuity of the router's cost.
    fn next_refill_budget(&self, i: usize, debit: U256, cost: U256) -> Option<U256> {
        let m = &self.market;
        let f = &self.fee_pair;
        let unit = U256::from(1000);
        let t = U256::from(f.tier);
        let principal = debit
            .checked_mul(debit)?
            .checked_div(debit.checked_add(cost)?)?;
        let nominal = m.price(i, principal, false)?;
        let final_fee = m.ceiling_fee(i, principal, nominal, false)?;
        let physical = f.balances[0].checked_sub(f.fee_reserve)?;
        let residual = physical.saturating_sub(f.reserves[0]);
        let internal = (residual.checked_mul(t)? / (unit + t))
            .checked_add(final_fee.checked_mul(t)? / unit)?;
        if internal < unit || final_fee > self.router_balance {
            return None;
        }
        let liquid = physical.checked_sub(final_fee)?.checked_sub(internal)?;
        let payment =
            mul_div_up(f.reserves[0], f.reserves[1], liquid)?.saturating_sub(f.balances[1]);
        if payment <= debit.checked_sub(principal)? {
            return None;
        }
        let old = payment - U256::ONE;
        // If cost stays constant, ceil(n*cost/(n+cost)) must reach payment.
        let fixed = cost
            .checked_sub(old)
            .filter(|d| !d.is_zero())
            .and_then(|divisor| {
                old.checked_mul(cost)?
                    .checked_div(divisor)?
                    .checked_add(U256::ONE)
            });
        // First nominal debit where floor(rW*raw/(rB-raw))+1 increases.
        let raw = mul_div_up(cost, f.reserves[0], f.reserves[1].checked_add(cost)?)?;
        let need_fee = mul_div_up(raw, unit, unit + t)?;
        let (side, rate) = m.oracle?;
        let scaled = mul_div_up(need_fee, U256::ONE << 112, rate)?;
        let tier = U256::from(m.tier);
        let next_cost = if side == i {
            mul_div_up(scaled, unit, tier)
        } else {
            m.minimum_principal(i, mul_div_up(scaled, unit + tier, tier)?, false)
        };
        let next = fixed.into_iter().chain(next_cost).min()?;
        (next > debit).then_some(next)
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
            if embedded {
                let physical = m.balances[0].checked_sub(m.fee_reserve)?;
                let surplus = physical.saturating_sub(m.reserves[0]);
                let minimum = if i == 0 {
                    m.reserves[0]
                        .checked_add(mul_div_up(unit, unit + tier, tier)?)?
                        .saturating_sub(physical)
                } else {
                    let fee_left = unit.saturating_sub(surplus.checked_mul(tier)? / (unit + tier));
                    m.minimum_principal(i, mul_div_up(fee_left, unit, tier)?, true)?
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
                let fee_tier = U256::from(fee_pair.tier);
                let surplus = fee_pair.balances[0]
                    .checked_sub(fee_pair.fee_reserve)?
                    .saturating_sub(fee_pair.reserves[0]);
                let fee_left =
                    unit.saturating_sub(surplus.checked_mul(fee_tier)? / (unit + fee_tier));
                let min_fee = mul_div_up(fee_left, unit, fee_tier)?.max(U256::ONE);
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
                    if lower > debit {
                        debit = lower;
                        continue;
                    }
                    if source == self.weth && self.exact_in(source, debit, false).is_none() {
                        debit = self.next_refill_budget(i, debit, cost)?;
                        continue;
                    }
                    break;
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

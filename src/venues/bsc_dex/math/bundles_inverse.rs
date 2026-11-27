//! Обратный расчёт входа. Каждая следующая нижняя граница следует из цены и комиссии при предыдущем объёме.
use super::{
    bundles::{Book, Fill, Market},
    full_math::{div_up, mul_div_up},
    swap_cl::Swapped,
};
use alloy_primitives::{Address, U256};

impl Market {
    /// Invert the embedded pair's physical K for the current output plateau. A larger input
    /// never decreases output or its BUN fee, so an input smaller than this bound also fails.
    fn embedded_input(&self, i: usize, mut n: U256) -> Option<U256> {
        let unit = U256::from(1000);
        let tier = U256::from(self.tier);
        let divisor = unit + tier;
        let k = self.reserves[0].checked_mul(self.reserves[1])?;
        let physical = self.balances[0].checked_sub(self.fee_reserve)?;
        let max = (U256::ONE << 112usize) - U256::ONE;
        if self.supply <= unit {
            return None;
        }
        let incoming = self.balances[1];
        let donation_fee = physical
            .saturating_sub(self.reserves[0])
            .checked_mul(tier)?
            / divisor;
        let (deficit, loss, gain, maximum) = if i == 0 {
            let lost = self.reserves[0]
                .saturating_sub(physical)
                .checked_mul(unit)?
                / divisor;
            (
                !lost.is_zero(),
                lost.saturating_sub(U256::ONE),
                U256::ONE,
                self.reserves[0]
                    .checked_add(max.checked_sub(self.reserves[0])?.checked_mul(divisor)? / unit)?
                    .checked_sub(physical)?,
            )
        } else {
            (
                self.reserves[1] >= incoming,
                self.reserves[1].saturating_sub(incoming),
                U256::ONE + div_up(divisor, unit)?,
                max.checked_sub(incoming)?,
            )
        };
        let outgoing = if i == 0 {
            incoming
        } else {
            physical.checked_sub(donation_fee)?
        };
        if deficit && (i != 0 || physical < self.reserves[0]) {
            // Let x be the nominal post-swap input reserve. Output rounding leaves strictly
            // less than K/x + slack, while the physical input reserve is at most x-loss.
            let slack = outgoing
                .checked_add(gain)?
                .checked_sub(self.reserves[1 - i])?;
            if slack.is_zero() {
                return None;
            }
            if !loss.is_zero() {
                // (x-loss)*(K/x+slack)>K requires x*(x-loss)>K*loss/slack.
                // Compare with 512-bit products, which fit for uint112 reserves and balances.
                let crosses = |x: U256| {
                    alloy_primitives::U512::from(x)
                        * alloy_primitives::U512::from(x.saturating_sub(loss))
                        * alloy_primitives::U512::from(slack)
                        > alloy_primitives::U512::from(k) * alloy_primitives::U512::from(loss)
                };
                let mut lo = self.reserves[i];
                let mut hi = if i == 0 {
                    lo.checked_add(maximum.checked_mul(unit)? / divisor)?
                } else {
                    lo.checked_add(maximum)?
                };
                if !crosses(hi) {
                    return None;
                }
                while lo < hi {
                    let mid = lo + (hi - lo) / U256::from(2);
                    if crosses(mid) {
                        hi = mid
                    } else {
                        lo = mid + U256::ONE
                    }
                }
                let growth = lo - self.reserves[i];
                n = n.max(if i == 0 {
                    mul_div_up(growth, divisor, unit)?
                } else {
                    growth
                });
            }
        }
        let excess = outgoing.saturating_sub(max);
        let min_credit = if i == 0 {
            excess
        } else {
            mul_div_up(excess, unit, divisor)?
        };
        if !min_credit.is_zero() {
            n = n.max(self.minimum_principal(i, min_credit, true)?)
        }
        while n <= maximum {
            let out = self.price(i, n, true)?;
            let required = if i == 0 {
                let retained = div_up(k, incoming.checked_sub(out)?)?;
                let extra = retained.saturating_sub(self.reserves[0]);
                if extra.is_zero() {
                    U256::ZERO
                } else {
                    self.reserves[0]
                        .checked_add((extra - U256::ONE).checked_mul(divisor)? / unit)?
                        .checked_add(U256::ONE)?
                        .saturating_sub(physical)
                }
            } else {
                let burned = out.checked_mul(tier)? / unit;
                let retained = outgoing.checked_sub(out)?.checked_sub(burned)?;
                div_up(k, retained)?.saturating_sub(incoming)
            };
            if required <= n {
                return Some(n);
            }
            n = required;
        }
        None
    }

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

/// First j in 0..len where floor((base+j*step+diff)*rate/Q) minus the
/// nominal floor fits free BUN. The comparison reduces to a bound on the
/// low 112 bits of the product. Prefix minima locate the first hit exactly.
fn first_covered(
    base: U256,
    step: U256,
    len: U256,
    diff: U256,
    rate: U256,
    free: U256,
) -> Option<U256> {
    let q = U256::ONE << 112usize;
    let (whole, remainder) = diff.checked_mul(rate)?.div_rem(q);
    if free < whole || len.is_zero() {
        return None;
    }
    if free > whole || remainder.is_zero() {
        return Some(U256::ZERO);
    }
    let upper = q - remainder;
    let r = rate % q;
    let a = (step % q) * r % q;
    let b = (base % q) * r % q;
    if b < upper {
        return Some(U256::ZERO);
    }
    let hit = |length| super::temple::min_residue(a, b, q, length) < upper;
    if !hit(len) {
        return None;
    }
    let mut lo = U256::ONE;
    let mut hi = len - U256::ONE;
    while lo < hi {
        let mid = lo + (hi - lo) / U256::from(2);
        if hit(mid + U256::ONE) {
            hi = mid;
        } else {
            lo = mid + U256::ONE;
        }
    }
    Some(lo)
}

impl Book {
    // Input required for the nominal BUN fee to reach a given integer amount.
    fn fee_input(&self, i: usize, fee: U256) -> Option<U256> {
        let m = &self.market;
        let (side, rate) = m.oracle?;
        let basis = mul_div_up(fee, U256::ONE << 112, rate)?;
        let unit = U256::from(1000);
        let tier = U256::from(m.tier);
        if i == side {
            mul_div_up(basis, unit, tier)
        } else {
            m.minimum_principal(i, mul_div_up(basis, unit + tier, tier)?, false)
        }
    }

    // The actual pair and the refill pair both independently enforce their 1,000 raw BUN minimum.
    fn market_fee_floor(&self, i: usize) -> Option<U256> {
        let m = &self.market;
        let (side, rate) = m.oracle?;
        let unit = U256::from(1000);
        let tier = U256::from(m.tier);
        let basis = mul_div_up(unit, U256::ONE << 112, rate)?;
        if side == i {
            Some(
                m.reserves[i]
                    .checked_add(mul_div_up(basis, unit, tier)?)?
                    .saturating_sub(m.balances[i]),
            )
        } else {
            let donation = m.balances[side].saturating_sub(m.reserves[side]);
            let left = basis.saturating_sub(donation.checked_mul(tier)? / unit);
            m.minimum_principal(i, mul_div_up(left, unit + tier, tier)?, false)
        }
    }

    // Find the first input whose paid BUN fee covers the measured arrival. The inner
    // raw-unit exchange rate can have period 2^112; use a modular range minimum for it.
    fn donated_fee_input(&self, i: usize, n: U256, out: U256, paid: U256) -> Option<U256> {
        let m = &self.market;
        let (side, rate) = m.oracle?;
        let q = U256::ONE << 112usize;
        let unit = U256::from(1000);
        let tier = U256::from(m.tier);
        let donation = m.balances[side].checked_sub(m.reserves[side])?;
        let free = m.balances[2].checked_sub(m.fee_reserve)?;
        let basis = if side == i {
            n.checked_mul(tier)? / unit
        } else {
            out.checked_mul(tier)? / (unit + tier)
        };
        let diff = if side == i {
            n.checked_add(donation)?.checked_mul(tier)? / unit - basis
        } else {
            donation.checked_mul(tier)? / unit
        };
        if (basis + diff).checked_mul(rate)? / q <= paid.checked_add(free)? {
            return None;
        }
        let max = (q - U256::ONE).checked_sub(m.balances[i])?;
        if n >= max {
            return None;
        }
        if i != side {
            // In this orientation the donation adds a constant to the output fee basis.
            let ceiling = m.price(i, max, false)?.checked_mul(tier)? / (unit + tier);
            let lo = basis.checked_add(U256::ONE)?;
            let delta = first_covered(
                lo,
                U256::ONE,
                ceiling.checked_sub(lo)? + U256::ONE,
                diff,
                rate,
                free,
            )?;
            return m.minimum_principal(i, mul_div_up(lo + delta, unit + tier, tier)?, false);
        }
        // At n = start+j*p, floor(n*tier/1000) advances by an integer and the
        // donation adds the same integer difference at every j. There are <=1000
        // rounding classes, independently of the oracle period and token amount.
        let mut g = 1000u16;
        let mut rem = m.tier;
        while rem != 0 {
            (g, rem) = (rem, g % rem);
        }
        let cycle = 1000 / g;
        let period = U256::from(cycle);
        let step = U256::from(m.tier / g);
        let mut best = None;
        for r in 1..=cycle {
            let start = n + U256::from(r);
            let end = best.map_or(max, |v: U256| v - U256::ONE);
            if start > end {
                break;
            }
            let b = start.checked_mul(tier)? / unit;
            let d = (start + donation).checked_mul(tier)? / unit - b;
            let len = (end - start) / period + U256::ONE;
            if let Some(offset) = first_covered(b, step, len, d, rate, free) {
                best = Some(start + offset * period);
            }
        }
        best
    }

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

    fn debit_for(principal: U256, cost: U256) -> Option<U256> {
        let square =
            principal.checked_mul(principal.checked_add(cost.checked_mul(U256::from(4))?)?)?;
        let root: U256 = square.root(2);
        let ceil = root + U256::from(u8::from(root * root != square));
        div_up(principal.checked_add(ceil)?, U256::from(2))
    }

    fn cost_change(&self, i: usize, cost: U256) -> Option<U256> {
        let f = &self.fee_pair;
        let raw = mul_div_up(cost, f.reserves[0], f.reserves[1].checked_add(cost)?)?;
        let unit = U256::from(1000);
        self.fee_input(i, mul_div_up(raw, unit, unit + U256::from(f.tier))?)
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
        let next_cost = self.cost_change(i, cost);
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
            if embedded {
                debit = m.embedded_input(i, debit)?;
            } else {
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
                let needed = self.fee_input(i, min_fee)?;
                principal = principal.max(needed).max(self.market_fee_floor(i)?);
                debit = principal;
                loop {
                    let gross = m.price(i, debit, false)?;
                    let fee = m.ceiling_fee(i, debit, gross, false)?;
                    let cost = self.refill_cost(fee)?;
                    let lower = if source == self.weth {
                        // n² ≥ principal·(n+cost). Taking the integer ceiling preserves the least debit.
                        Self::debit_for(principal, cost)?
                    } else {
                        m.minimum_principal(i, amount.checked_add(cost)?, false)?
                            .max(principal)
                    };
                    if lower > debit {
                        debit = lower;
                        continue;
                    }
                    if self.exact_in(source, debit, false).is_some() {
                        break;
                    }
                    if source == self.weth {
                        let current = debit
                            .checked_mul(debit)?
                            .checked_div(debit.checked_add(cost)?)?;
                        let output = m.price(i, current, false)?;
                        let paid = m.ceiling_fee(i, current, output, false)?;
                        if let Some(next) = self.donated_fee_input(i, current, output, paid) {
                            // The adjusted principal is monotone while cost is constant. At a new
                            // fee level it may drop, so revisit that level before skipping farther.
                            debit = Self::debit_for(next, cost)
                                .into_iter()
                                .chain(self.cost_change(i, cost))
                                .min()
                                .filter(|step| *step > debit)?;
                        } else {
                            debit = self.next_refill_budget(i, debit, cost)?;
                        }
                    } else {
                        debit = self.donated_fee_input(i, debit, gross, fee)?;
                    }
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

#[cfg(test)]
#[path = "bundles_inverse_tests.rs"]
mod tests;

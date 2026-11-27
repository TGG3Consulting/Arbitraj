//! Ethereum BPool: the balances used for pricing and the tokens held for payment.
//! Source: balancer/balancer-core@f4ed5d65362a8d6cec21662fb6eae233b0babc1f, BNum/BMath/BPool.
use alloy_primitives::{Address, U256};
use serde::{Deserialize, Serialize};

const UNIT: u64 = 1_000_000_000_000_000_000;
const PRECISION: u64 = UNIT / 10_000_000_000;
const MAX_IN_RATIO: u64 = UNIT / 2;
const MAX_OUT_RATIO: u64 = UNIT / 3 + 1;

fn unit() -> U256 {
    U256::from(UNIT)
}

// BNum checks overflow before dividing, then rounds towards the nearest integer.
fn mul(a: U256, b: U256) -> Option<U256> {
    a.checked_mul(b)?
        .checked_add(U256::from(UNIT / 2))?
        .checked_div(unit())
}
fn div(a: U256, b: U256) -> Option<U256> {
    a.checked_mul(unit())?
        .checked_add(b / U256::from(2))?
        .checked_div(b)
}

fn difference(a: U256, b: U256) -> U256 {
    if a >= b { a - b } else { b - a }
}

fn pow(base: U256, exponent: U256) -> Option<U256> {
    if base.is_zero() || base >= U256::from(2 * UNIT) {
        return None;
    }
    let mut n = exponent / unit();
    let mut factor = base;
    let mut whole = unit();
    while !n.is_zero() {
        if n.bit(0) {
            whole = mul(whole, factor)?;
        }
        n >>= 1;
        if !n.is_zero() {
            factor = mul(factor, factor)?;
        }
    }
    let fractional = exponent % unit();
    if fractional.is_zero() {
        return Some(whole);
    }
    let x = difference(base, unit());
    let mut previous_k = U256::ZERO;
    let mut term = unit();
    let mut sum = unit();
    let mut negative = false;
    while term >= U256::from(PRECISION) {
        let coefficient = difference(fractional, previous_k);
        negative ^= (base < unit()) ^ (fractional < previous_k);
        previous_k = previous_k.checked_add(unit())?;
        term = div(mul(term, mul(coefficient, x)?)?, previous_k)?;
        sum = if negative {
            sum.checked_sub(term)?
        } else {
            sum.checked_add(term)?
        };
    }
    mul(whole, sum)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Asset {
    pub token: Address,
    pub recorded: U256,
    pub physical: U256,
    #[serde(default)]
    pub exact_transfer: bool,
    pub weight: U256,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct State {
    pub pool: Address,
    pub assets: Vec<Asset>,
    pub fee: U256,
    pub public_swap: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fill {
    pub spent: U256,
    pub received: U256,
    pub spot_after: U256,
}

fn spot(x: U256, wx: U256, y: U256, wy: U256, fee: U256) -> Option<U256> {
    let ratio = div(div(x, wx)?, div(y, wy)?)?;
    mul(ratio, div(unit(), unit().checked_sub(fee)?)?)
}

impl State {
    fn indices(&self, input: Address, output: Address) -> Option<(usize, usize)> {
        if input == output {
            return None;
        }
        Some((
            self.assets.iter().position(|a| a.token == input)?,
            self.assets.iter().position(|a| a.token == output)?,
        ))
    }

    /// Raw input per output, scaled by 10^18. Matches BPool.getSpotPrice including fee.
    pub fn spot_price(&self, input: Address, output: Address) -> Option<U256> {
        let (i, j) = self.indices(input, output)?;
        let (x, y) = (&self.assets[i], &self.assets[j]);
        spot(x.recorded, x.weight, y.recorded, y.weight, self.fee)
    }

    /// The pure BMath return value, before cash, size and spot-price checks in BPool.
    pub fn amount(
        &self,
        input: Address,
        output: Address,
        qty: U256,
        exact_in: bool,
    ) -> Option<U256> {
        let (i, j) = self.indices(input, output)?;
        let (x, y) = (&self.assets[i], &self.assets[j]);
        let net_fee = unit().checked_sub(self.fee)?;
        if exact_in {
            let paid = mul(qty, net_fee)?;
            let price = div(x.recorded, x.recorded.checked_add(paid)?)?;
            let exponent = div(x.weight, y.weight)?;
            mul(y.recorded, unit().checked_sub(pow(price, exponent)?)?)
        } else {
            let price = div(y.recorded, y.recorded.checked_sub(qty)?)?;
            let exponent = div(y.weight, x.weight)?;
            div(
                mul(x.recorded, pow(price, exponent)?.checked_sub(unit())?)?,
                net_fee,
            )
        }
    }

    /// A nominal swap that passes this BPool's arithmetic and public-swap bounds.
    /// The caller still checks wallet transfer rules, minimum output and maximum spend.
    pub fn swap(
        &self,
        input: Address,
        output: Address,
        qty: U256,
        exact_in: bool,
        max_price: U256,
    ) -> Option<Fill> {
        let (i, j) = self.indices(input, output)?;
        let (x, y) = (&self.assets[i], &self.assets[j]);
        if !self.public_swap || qty.is_zero() {
            return None;
        }
        let cap = if exact_in {
            mul(x.recorded, U256::from(MAX_IN_RATIO))?
        } else {
            mul(y.recorded, U256::from(MAX_OUT_RATIO))?
        };
        if qty > cap {
            return None;
        }
        let quoted = self.amount(input, output, qty, exact_in)?;
        let (spent, received) = if exact_in {
            (qty, quoted)
        } else {
            (quoted, qty)
        };
        if y.physical < received {
            return None;
        }
        let before = self.spot_price(input, output)?;
        let after = spot(
            x.recorded.checked_add(spent)?,
            x.weight,
            y.recorded.checked_sub(received)?,
            y.weight,
            self.fee,
        )?;
        (before <= after && after <= max_price && before <= div(spent, received)?).then_some(Fill {
            spent,
            received,
            spot_after: after,
        })
    }

    /// Highest by-source quote within the price band before checking average-price
    /// rounding. The exact swap check supplies the measured depth; the caller marks
    /// it as a lower bound when a rounded boundary falls in an unavailable interval.
    pub fn band(&self, input: Address, output: Address, limit: U256) -> Option<Fill> {
        let (i, j) = self.indices(input, output)?;
        let (x, y) = (&self.assets[i], &self.assets[j]);
        if !self.public_swap {
            return None;
        }
        let mut lo = U256::ZERO;
        let mut hi = mul(x.recorded, U256::from(MAX_IN_RATIO))?;
        while lo < hi {
            let mid = lo + (hi - lo).div_ceil(U256::from(2));
            let allowed = self.amount(input, output, mid, true).and_then(|q| {
                let price = spot(
                    x.recorded.checked_add(mid)?,
                    x.weight,
                    y.recorded.checked_sub(q)?,
                    y.weight,
                    self.fee,
                )?;
                Some(q <= y.physical && price <= limit)
            });
            if allowed == Some(true) {
                lo = mid;
            } else {
                hi = mid - U256::ONE;
            }
        }
        if let Some(fill) = self.swap(input, output, lo, true, limit) {
            return Some(fill);
        }
        let output_at_limit = self.amount(input, output, lo, true)?;
        // A rejected rounded maximum can still have a funded by-target payout.
        // The returned forward quote always uses by-source and stays within the band.
        let candidate =
            self.forward_for_output(input, output, output_at_limit.saturating_sub(U256::ONE))?;
        (candidate.spent <= lo && candidate.spot_after <= limit).then_some(candidate)
    }

    /// Least input for a by-source swap paying at least `want`. The calculator is
    /// monotone: for base <= 1 each fractional-series term is subtracted and its
    /// magnitude is monotone in 1-base, including the stopping threshold.
    /// The BPool average-price check can create holes even on a nonzero plateau.
    pub fn forward_for_output(&self, input: Address, output: Address, want: U256) -> Option<Fill> {
        let (i, j) = self.indices(input, output)?;
        let (x, y) = (&self.assets[i], &self.assets[j]);
        if !self.public_swap || want.is_zero() || want > y.physical {
            return None;
        }
        let cap = mul(x.recorded, U256::from(MAX_IN_RATIO))?;
        let before = self.spot_price(input, output)?;
        let credit = |n| self.amount(input, output, n, true);
        let mut lo = U256::ONE;
        let mut hi = self
            .amount(input, output, want, false)
            .unwrap_or(cap)
            .clamp(lo, cap.max(lo));
        if hi > cap {
            return None;
        }
        while credit(hi)? < want {
            if hi == cap {
                return None;
            }
            lo = hi.checked_add(U256::ONE)?;
            hi = hi.saturating_mul(U256::from(2)).min(cap);
        }
        while lo < hi {
            let mid = lo + ((hi - lo) >> 1);
            if credit(mid)? < want {
                lo = mid + U256::ONE;
            } else {
                hi = mid;
            }
        }
        loop {
            let out = credit(lo)?;
            if out > y.physical {
                return None;
            }
            // BNum bdiv(lo,out)=(lo*UNIT+floor(out/2))/out. All higher
            // quotes have at least this output, so every skipped input fails too.
            let minimum = before
                .checked_mul(out)?
                .saturating_sub(out / U256::from(2))
                .div_ceil(unit());
            if lo >= minimum {
                // Increasing input and decreasing recorded output can only increase
                // the BPool spot price. Negative impact is impossible without overflow.
                return self.swap(input, output, lo, true, U256::MAX);
            }
            if minimum > cap {
                return None;
            }
            lo = minimum;
        }
    }

    /// Apply a swap only when standard transfers on this edge have been established.
    pub fn apply(&self, input: Address, output: Address, fill: Fill) -> Option<Self> {
        let (i, j) = self.indices(input, output)?;
        if !self.assets[i].exact_transfer || !self.assets[j].exact_transfer {
            return None;
        }
        let mut next = self.clone();
        next.assets[i].recorded = next.assets[i].recorded.checked_add(fill.spent)?;
        next.assets[i].physical = next.assets[i].physical.checked_add(fill.spent)?;
        next.assets[j].recorded = next.assets[j].recorded.checked_sub(fill.received)?;
        next.assets[j].physical = next.assets[j].physical.checked_sub(fill.received)?;
        Some(next)
    }
}

#[cfg(test)]
#[path = "balancer_v1_tests.rs"]
mod tests;

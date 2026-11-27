//! YieldBasis LEVAMM, crvUSD against the Curve pool LP token.
//! Arithmetic from the deployed Vyper versions; safe limits differ by generation.
use alloy_primitives::{Address, B256, U256};
use serde::{Deserialize, Serialize};
const UNIT: u64 = 1_000_000_000_000_000_000;
fn unit() -> U256 {
    U256::from(UNIT)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Limits {
    Always,
    RelaxImproving,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LevState {
    pub index: u64,
    pub amm: Address,
    pub stable: Address,
    pub lp: Address,
    pub oracle: Address,
    pub lt: Address,
    pub collateral: U256,
    pub debt: U256,
    pub price: U256,
    pub precision: U256,
    pub fee: U256,
    pub cash: [U256; 2],
    pub limits: Limits,
    pub killed: bool,
}
/// Official get_dy points for Curve/Virtual. Preserve registry identity alongside
/// the samples so a cached state cannot be reused under another physical venue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Preview {
    pub id: B256,
    pub venue: Address,
    pub cryptopool: Address,
    pub amm: Address,
    pub coins: [Address; 2],
    pub levels: [Vec<super::LadderPoint>; 2],
}
impl Preview {
    pub fn points(&self, input: Address) -> Option<&[super::LadderPoint]> {
        self.coins
            .iter()
            .position(|v| *v == input)
            .map(|i| self.levels[i].as_slice())
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fill {
    pub spent: U256,
    pub received: U256,
}

impl LevState {
    fn side(&self, input: Address) -> Option<usize> {
        [self.stable, self.lp].iter().position(|t| *t == input)
    }
    fn value_numerator(&self, collateral: U256) -> Option<U256> {
        self.price
            .checked_mul(collateral)?
            .checked_mul(self.precision)
    }
    fn x0(&self, collateral: U256, debt: U256, safe: bool) -> Option<U256> {
        // Both deployed versions enforce precisely 2x leverage for this implementation.
        let value = self.value_numerator(collateral)? / unit();
        if safe
            && (debt < value.checked_mul(U256::from(UNIT / 16))? / unit()
                || debt > value.checked_mul(U256::from(17 * (UNIT / 32)))? / unit())
        {
            return None;
        }
        let ratio = U256::from((4_u128 * u128::from(UNIT)) / 9);
        let left = value.checked_mul(value)?;
        let coefficient = value.checked_mul(U256::from(4))?.checked_mul(ratio)? / unit();
        let disc = left.checked_sub(coefficient.checked_mul(debt)?)?;
        value
            .checked_add(disc.root(2))?
            .checked_mul(unit())?
            .checked_div(ratio.checked_mul(U256::from(2))?)
    }
    pub fn x(&self) -> Option<U256> {
        self.x0(self.collateral, self.debt, false)?
            .checked_sub(self.debt)
    }
    pub fn reserves(&self, input: Address) -> Option<(U256, U256)> {
        let x = self.x()?;
        Some(if self.side(input)? == 0 {
            (x, self.collateral)
        } else {
            (self.collateral, x)
        })
    }
    /// The original get_dy view with price() and the time-adjusted debt of this snapshot.
    fn gross_output(&self, input: Address, qty: U256) -> Option<U256> {
        if input == self.stable && qty > self.debt {
            return None;
        }
        let (x, y) = self.reserves(input)?;
        let denom = x.checked_add(qty)?;
        if denom.is_zero() {
            return None;
        }
        let next = x.checked_mul(y)?.div_ceil(denom);
        y.checked_sub(next)
    }
    pub fn amount(&self, input: Address, qty: U256) -> Option<U256> {
        self.gross_output(input, qty)?
            .checked_mul(unit().checked_sub(self.fee)?)?
            .checked_div(unit())
    }
    pub fn output_fee(&self, input: Address, qty: U256) -> Option<U256> {
        self.gross_output(input, qty)?
            .checked_sub(self.amount(input, qty)?)
    }
    fn ratio(&self, c: U256, d: U256) -> Option<U256> {
        // Vyper evaluates the numerator even when unsafe_div by zero returns zero.
        let n = self.value_numerator(c)?;
        Some(if d.is_zero() { U256::MAX } else { n / d })
    }
    /// A candidate at the current snapshot's oracle price; final execution also
    /// calls price_w(), collects LT fees and must pass the actual payer simulation.
    pub fn swap(&self, input: Address, qty: U256) -> Option<Fill> {
        let side = self.side(input)?;
        let received = self.amount(input, qty)?;
        if self.killed
            || self.collateral.is_zero()
            || received.is_zero()
            || received > self.cash[1 - side]
        {
            return None;
        }
        let (c, d) = if side == 0 {
            (
                self.collateral.checked_sub(received)?,
                self.debt.checked_sub(qty)?,
            )
        } else {
            (
                self.collateral.checked_add(qty)?,
                self.debt.checked_add(received)?,
            )
        };
        let safe = match self.limits {
            Limits::Always => true,
            Limits::RelaxImproving => {
                let before = self.ratio(self.collateral, self.debt)?;
                let after = self.ratio(c, d)?;
                if after > U256::from(2 * UNIT) {
                    before <= after
                } else {
                    before >= after
                }
            }
        };
        (self.x0(c, d, safe)? >= self.x0(self.collateral, self.debt, false)?).then_some(Fill {
            spent: qty,
            received,
        })
    }
    /// Analytic inverse of the single exact-source wire, followed by all state checks.
    pub fn forward_for_output(&self, input: Address, want: U256) -> Option<Fill> {
        let (x, y) = self.reserves(input)?;
        let keep = unit().checked_sub(self.fee)?;
        if want.is_zero() || keep.is_zero() {
            return None;
        }
        let gross = want.checked_mul(unit())?.div_ceil(keep);
        let remaining = y.checked_sub(gross)?;
        if remaining.is_zero() {
            return None;
        }
        let spent = x.checked_mul(y)?.div_ceil(remaining).checked_sub(x)?;
        self.swap(input, spent).filter(|f| f.received >= want)
    }
}

#[cfg(test)]
#[path = "yield_basis_tests.rs"]
mod tests;

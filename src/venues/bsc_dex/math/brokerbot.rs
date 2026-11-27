//! Линейная кривая для целых акций. Старые Market делят цену на два до умножения на число акций.
use super::swap_cl::Swapped;
use crate::core::types::BrokerbotState;
use alloy_primitives::{Address, U256};

/// Максимальное целое с монотонным предикатом на [0,limit]; ноль разрешён по соглашению.
fn max_count(limit: U256, accept: impl Fn(U256) -> bool) -> U256 {
    let mut lo = U256::ZERO;
    let mut hi = limit;
    while lo < hi {
        let distance = hi - lo;
        let mid = hi - (distance >> 1usize);
        if accept(mid) {
            lo = mid;
        } else {
            hi = mid - U256::ONE;
        }
    }
    lo
}
impl BrokerbotState {
    pub(crate) fn gross(&self, buy: bool, units: U256) -> Option<U256> {
        if units.is_zero() {
            return Some(U256::ZERO);
        }
        let lowest = if buy {
            self.price
        } else {
            self.price.checked_sub(units.checked_mul(self.increment)?)?
        };
        let highest = lowest.checked_add((units - U256::ONE).checked_mul(self.increment)?)?;
        let sum = lowest.checked_add(highest)?;
        if self.average_first {
            (sum >> 1usize).checked_mul(units)
        } else {
            Some(sum.checked_mul(units)? >> 1usize)
        }
    }
    pub(crate) fn license(&self, gross: U256) -> Option<U256> {
        Some(gross.checked_mul(U256::from(self.license_bps))? / U256::from(10000))
    }
    fn sale_limit(&self) -> Option<U256> {
        let shares = self.supply.checked_sub(self.share_balance)?;
        Some(if self.increment.is_zero() {
            shares
        } else {
            shares.min(self.price.min(self.base_price) / self.increment)
        })
    }
    fn sell(&self, n: U256) -> Option<U256> {
        let gross = self.gross(false, n)?;
        if gross > self.cash_balance {
            return None;
        }
        gross.checked_sub(self.license(gross)?)
    }
    /// Для старой формулы округление средней цены может нарушить монотонность между соседними
    /// размерами. При фиксированной чётности оба предиката монотонны в зоне неотрицательной цены.
    fn parity_sale(&self, wanted: Option<U256>) -> Option<U256> {
        let max = self.sale_limit()?;
        let mut chosen = None;
        for first in [U256::ONE, U256::from(2)] {
            if first > max || self.sell(first).is_none() {
                continue;
            }
            let step = |i: U256| first + (i << 1usize);
            let last = max_count((max - first) >> 1usize, |i| self.sell(step(i)).is_some());
            let n = if let Some(target) = wanted {
                if self.sell(step(last))? < target {
                    continue;
                }
                if self.sell(first)? >= target {
                    first
                } else {
                    step(
                        max_count(last, |i| self.sell(step(i)).is_some_and(|x| x < target))
                            + U256::ONE,
                    )
                }
            } else {
                step(last)
            };
            chosen = Some(chosen.map_or(n, |prev: U256| {
                if wanted.is_some() {
                    prev.min(n)
                } else {
                    prev.max(n)
                }
            }));
        }
        chosen
    }
    pub(crate) fn max_units(&self, buy: bool) -> Option<U256> {
        if buy && self.buying {
            Some(self.share_balance)
        } else if !buy && self.selling {
            self.parity_sale(None)
        } else {
            None
        }
    }
    pub(crate) fn swap(&self, input: Address, exact_in: bool, amount: U256) -> Option<Swapped> {
        if self.price.is_zero() || amount.is_zero() {
            return None;
        }
        let (spent, net) = if input == self.cash && self.buying {
            let n = if exact_in {
                max_count(self.share_balance.min(amount / self.price), |n| {
                    self.gross(true, n).is_some_and(|v| v <= amount)
                })
            } else {
                amount
            };
            if n > self.share_balance {
                return None;
            }
            self.base_price
                .checked_add(n.checked_mul(self.increment)?)?;
            (self.gross(true, n)?, n)
        } else if input == self.share && self.selling {
            let n = if exact_in {
                amount
            } else {
                self.parity_sale(Some(amount))?
            };
            if n > self.sale_limit()? {
                return None;
            }
            (n, self.sell(n)?)
        } else {
            return None;
        };
        if spent.is_zero() || net.is_zero() {
            return None;
        }
        Some(Swapped {
            amount_in: spent,
            amount_out: net,
            complete: true,
        })
    }
    pub(crate) fn after(&self, input: Address, amount: U256) -> Option<Self> {
        let fill = self.swap(input, true, amount)?;
        let mut next = self.clone();
        if input == self.cash {
            let change = fill.amount_out.checked_mul(next.increment)?;
            next.price = next.price.checked_add(change)?;
            next.base_price = next.base_price.checked_add(change)?;
            next.cash_balance = next.cash_balance.checked_add(fill.amount_in)?;
            next.share_balance = next.share_balance.checked_sub(fill.amount_out)?;
        } else {
            let change = fill.amount_in.checked_mul(next.increment)?;
            next.price = next.price.checked_sub(change)?;
            next.base_price = next.base_price.checked_sub(change)?;
            next.cash_balance = next
                .cash_balance
                .checked_sub(self.gross(false, fill.amount_in)?)?;
            next.share_balance = next.share_balance.checked_add(fill.amount_in)?;
        }
        Some(next)
    }
}

#[cfg(test)]
#[path = "brokerbot_tests.rs"]
mod tests;

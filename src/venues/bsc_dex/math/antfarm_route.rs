//! Size an Antfarm trade in the currency the caller can actually spend.
//! For a separate ATF fee, include a real exact-output funding purchase on the same block.
use super::{
    antfarm::{Fill, Market},
    full_math::mul_div_up,
    swap_cl::Swapped,
};
use crate::core::types::AntfarmBook;
use alloy_primitives::{Address, U256};

#[derive(Debug, Clone)]
pub struct Route {
    pub primary: Fill,
    pub acquisition: Option<Fill>,
    pub funding_tier: u16,
}
impl Route {
    pub fn debit(&self) -> Option<U256> {
        self.primary
            .debit
            .checked_add(self.acquisition.as_ref().map_or(U256::ZERO, |v| v.debit))
    }
    pub fn swapped(&self, exact_in: bool, requested: U256) -> Option<Swapped> {
        Some(Swapped {
            amount_in: self.debit()?,
            amount_out: self.primary.output,
            complete: if exact_in {
                self.debit()? == requested
            } else {
                self.primary.output >= requested
            },
        })
    }
}

impl Market {
    /// Smallest input budget that the standard exact-input router maps to at least 'want'.
    pub(super) fn minimum_for(&self, i: usize, want: U256) -> Option<U256> {
        let den = U256::from(1000);
        let t = U256::from(self.tier);
        let embedded = self.tokens[0] == self.atf;
        let raw = if embedded && i == 1 {
            mul_div_up(want, den, den.checked_sub(t)?)?
        } else {
            want
        };
        let net = mul_div_up(
            self.reserves[i],
            raw,
            self.reserves[1 - i].checked_sub(raw)?,
        )?;
        if embedded && i == 0 {
            let gross = mul_div_up(net, den + t, den)?;
            let parameter = mul_div_up(gross, den, den + t)?;
            Some(parameter.checked_mul(den + t)? / den)
        } else {
            Some(net)
        }
    }
    /// Output of an ordinary separate-fee AMM and the router's third-token charge. Both are nondecreasing with x.
    fn ordinary(&self, i: usize, x: U256) -> Option<(U256, U256)> {
        let (oi, rate) = self.oracle?;
        let out = x.checked_mul(self.reserves[1 - i])? / self.reserves[i].checked_add(x)?;
        let in_or_out = if oi == i { x } else { out };
        let basis = in_or_out.checked_mul(U256::from(self.tier))? / U256::from(1000);
        Some((out, rate.checked_mul(basis)? >> 112))
    }
    fn purchase_cost(&self, target: U256) -> Option<U256> {
        if target.is_zero() {
            return Some(U256::ZERO);
        }
        let gross = target
            .checked_mul(U256::from(1000))?
            .checked_div(U256::from(1000u16.checked_sub(self.tier)?))?;
        self.reserves[1]
            .checked_mul(gross)?
            .checked_div(self.reserves[0].checked_sub(gross)?)?
            .checked_add(U256::ONE)
    }
}

impl AntfarmBook {
    /// Carry both consumed curves into the next leg in the same block.
    pub fn after(&self, source: Address, amount: U256) -> Option<(Swapped, Self)> {
        let (selected, _) = self.choose_payment(source, true, amount)?;
        if let Some((tier, funds)) = selected {
            let f = self.output_at(source, true, amount, tier, funds)?;
            let mut next = self.clone();
            let swapped = Swapped {
                amount_in: f.primary.debit,
                amount_out: f.credit,
                complete: f.primary.debit == amount,
            };
            next.market = f.primary.next;
            let (src, _) = next
                .funding
                .iter_mut()
                .find(|(m, _)| m.tokens == f.refill.next.tokens && m.tier == tier)?;
            *src = f.refill.next;
            return Some((swapped, next));
        }
        let flow = self.route(source, true, amount)?;
        let fill = flow.swapped(true, amount)?;
        let mut next = self.clone();
        next.market = flow.primary.next;
        if let Some(acquired) = flow.acquisition {
            let (market, _) = next
                .funding
                .iter_mut()
                .find(|(m, _)| m.tokens == acquired.next.tokens && m.tier == flow.funding_tier)?;
            *market = acquired.next;
        }
        Some((fill, next))
    }
    fn choose_payment(
        &self,
        input: Address,
        exact_in: bool,
        size: U256,
    ) -> Option<(Option<(u16, crate::core::types::AntfarmFunds)>, Swapped)> {
        let output_fill = |tier, funds| {
            let f = self.output_at(input, exact_in, size, tier, funds)?;
            Some(Swapped {
                amount_in: f.primary.debit,
                amount_out: f.credit,
                complete: if exact_in {
                    f.primary.debit == size
                } else {
                    f.credit >= size
                },
            })
        };
        if let Some((tier, funds)) = self.proceeds {
            return Some((self.proceeds, output_fill(tier, funds)?));
        }
        let mut best = self
            .route(input, exact_in, size)
            .and_then(|r| r.swapped(exact_in, size))
            .map(|s| (None, s));
        let Some(limits) = self.limits else {
            return best;
        };
        let target = self.market.tokens.iter().position(|t| *t != input)?;
        let output = self.market.tokens[target];
        for (fund, _) in &self.funding {
            if fund.tokens != [self.market.atf, output] {
                continue;
            }
            for budget in [limits.stock, limits.loans[target]].into_iter().flatten() {
                let Some(s) = output_fill(fund.tier, budget) else {
                    continue;
                };
                let improves = best.as_ref().is_none_or(|(_, b)| {
                    if exact_in {
                        (s.amount_out, s.complete, std::cmp::Reverse(s.amount_in))
                            > (b.amount_out, b.complete, std::cmp::Reverse(b.amount_in))
                    } else {
                        (s.amount_in, std::cmp::Reverse(s.amount_out))
                            < (b.amount_in, std::cmp::Reverse(b.amount_out))
                    }
                });
                if improves {
                    best = Some((Some((fund.tier, budget)), s));
                }
            }
        }
        best
    }
    pub fn swap(&self, input: Address, exact_in: bool, size: U256) -> Option<Swapped> {
        self.choose_payment(input, exact_in, size)
            .map(|(_, fill)| fill)
    }
    /// The smaller of two equal-output trades leaves more funds for another opportunity.
    pub fn route(&self, source: Address, exact_in: bool, size: U256) -> Option<Route> {
        let market = &self.market;
        let i = market.tokens.iter().position(|v| *v == source)?;
        if size.is_zero() {
            return None;
        }
        if market.tokens.contains(&market.atf) {
            let x = if exact_in {
                size
            } else {
                market.minimum_for(i, size)?
            };
            let primary = market.settle(source, x, false)?;
            return (exact_in || primary.output >= size).then_some(Route {
                primary,
                acquisition: None,
                funding_tier: 0,
            });
        }
        let mut best: Option<Route> = None;
        for (fund, projected) in &self.funding {
            if fund.tokens != [market.atf, source] {
                continue;
            }
            let mut current = market.clone();
            if let Some(rate) = projected
                && let Some((idx, _)) = market.oracle
                && idx == i
            {
                current.oracle = Some((idx, *rate));
            }
            let affordable = |x: U256| {
                current
                    .ordinary(i, x)
                    .and_then(|(_, f)| fund.purchase_cost(f))
                    .and_then(|c| c.checked_add(x))
                    .is_some_and(|v| v <= size)
            };
            let x = if exact_in {
                if affordable(size) {
                    size
                } else {
                    let mut low = U256::ZERO;
                    let mut high = size;
                    while high - low > U256::ONE {
                        let mid = low + (high - low) / U256::from(2);
                        if affordable(mid) {
                            low = mid;
                        } else {
                            high = mid;
                        }
                    }
                    low
                }
            } else {
                let Some(x) = current.minimum_for(i, size) else {
                    continue;
                };
                x
            };
            let Some((out, _)) = current.ordinary(i, x) else {
                continue;
            };
            let low = current.minimum_for(i, out).unwrap_or(x);
            for amount in [x, low] {
                let Some(primary) = current.settle(source, amount, false) else {
                    continue;
                };
                let Some(fee) = fund.acquire_fee(source, primary.separate_atf) else {
                    continue;
                };
                let Some(cost) = primary.debit.checked_add(fee.debit) else {
                    continue;
                };
                if (exact_in && cost > size) || (!exact_in && primary.output < size) {
                    continue;
                }
                let improve = best.as_ref().is_none_or(|b| {
                    let old = b.debit().unwrap_or(U256::MAX);
                    if exact_in {
                        (primary.output, cost == size, std::cmp::Reverse(cost))
                            > (b.primary.output, old == size, std::cmp::Reverse(old))
                    } else {
                        (cost, std::cmp::Reverse(primary.output))
                            < (old, std::cmp::Reverse(b.primary.output))
                    }
                });
                if improve {
                    best = Some(Route {
                        primary,
                        acquisition: Some(fee),
                        funding_tier: fund.tier,
                    });
                }
                if low == x {
                    break;
                }
            }
        }
        best
    }
}

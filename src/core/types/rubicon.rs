//! Escrowed offers of the Rubicon Classic matching engine. All values are raw ERC-20 units.
use alloy_primitives::{Address, U256};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Offer {
    pub id: U256,
    pub available: u128,
    pub requested: u128,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Book {
    pub market: Address,
    pub tokens: [Address; 2],
    /// Index of the token received by the maker. Offers stay in the contract's linked-list order.
    pub offers: [Vec<Offer>; 2],
    pub fees: [u32; 2],
    pub dust: [U256; 2],
}

pub struct Plan {
    pub actions: Vec<(U256, u128)>,
    pub spent: U256,
    pub received: U256,
    pub fees: U256,
    pub complete: bool,
}

impl Offer {
    /// buy(id, quantity) computes the maker's principal first, then floors each fee independently.
    fn amounts(&self, quantity: u128, fees: [u32; 2]) -> Option<(U256, U256)> {
        if self.available == 0 || quantity > self.available {
            return None;
        }
        let principal =
            U256::from(quantity) * U256::from(self.requested) / U256::from(self.available);
        let denominator = U256::from(100_000);
        let charge = fees
            .into_iter()
            .map(|f| principal * U256::from(f) / denominator)
            .sum::<U256>();
        Some((principal, charge))
    }

    fn affordable(&self, budget: U256, fees: [u32; 2]) -> Option<u128> {
        let fits = |n| self.amounts(n, fees).is_some_and(|(p, f)| p + f <= budget);
        if fits(self.available) {
            return Some(self.available);
        }
        let (mut low, mut high) = (0, self.available);
        while low < high {
            let mid = low + (high - low).div_ceil(2);
            if fits(mid) { low = mid } else { high = mid - 1 }
        }
        Some(low)
    }
}

impl Book {
    pub fn plan(&self, input: Address, exact_in: bool, amount: U256) -> Option<Plan> {
        let side = self.tokens.iter().position(|t| *t == input)?;
        let mut plan = Plan {
            actions: Vec::new(),
            spent: U256::ZERO,
            received: U256::ZERO,
            fees: U256::ZERO,
            complete: false,
        };
        for offer in &self.offers[side] {
            let remainder = if exact_in {
                amount - plan.spent
            } else {
                amount - plan.received
            };
            if remainder.is_zero() {
                break;
            }
            let quantity = if exact_in {
                offer.affordable(remainder, self.fees)?
            } else {
                remainder.min(U256::from(offer.available)).to::<u128>()
            };
            let (principal, fee) = offer.amounts(quantity, self.fees)?;
            // A zero debit cannot consume a maker offer. The protected wallet can select the next ID directly.
            if principal.is_zero() {
                continue;
            }
            plan.actions.push((offer.id, quantity));
            plan.spent = plan.spent.checked_add(principal + fee)?;
            plan.received = plan.received.checked_add(U256::from(quantity))?;
            plan.fees = plan.fees.checked_add(fee)?;
        }
        plan.complete = if exact_in {
            plan.spent == amount
        } else {
            plan.received == amount
        };
        (!plan.actions.is_empty()).then_some(plan)
    }

    /// The depth admission rule counts principal before fees, in settlement-token units.
    pub fn depth_in_band(&self, input: Address, buy: bool, band: u64, scale: u64) -> Option<U256> {
        use alloy_primitives::U512;
        let i = self.tokens.iter().position(|&t| t == input)?;
        let offers = &self.offers[i];
        let Some(best) = offers.first() else {
            return Some(U256::ZERO);
        };
        if scale == 0 || band >= scale {
            return None;
        }
        let mut cash = U256::ZERO;
        for offer in offers {
            let ask = U512::from(offer.requested) * U512::from(best.available);
            let reference = U512::from(best.requested) * U512::from(offer.available);
            let inside = if buy {
                ask * U512::from(scale) <= reference * (U512::from(scale) + U512::from(band))
            } else {
                ask * U512::from(scale - band) <= reference * U512::from(scale)
            };
            if inside {
                cash = cash.checked_add(U256::from(if buy {
                    offer.requested
                } else {
                    offer.available
                }))?;
            }
        }
        Some(cash)
    }

    pub fn after(&self, input: Address, p: &Plan) -> Option<Self> {
        let side = self.tokens.iter().position(|t| *t == input)?;
        let mut next = self.clone();
        for (id, quantity) in &p.actions {
            let offer = next.offers[side].iter_mut().find(|o| o.id == *id)?;
            let (principal, _) = offer.amounts(*quantity, self.fees)?;
            offer.available = offer.available.checked_sub(*quantity)?;
            offer.requested = offer.requested.checked_sub(principal.try_into().ok()?)?;
        }
        next.offers[side].retain(|o| {
            o.available != 0 && o.requested != 0 && U256::from(o.available) >= self.dust[1 - side]
        });
        Some(next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn real_two_offer_book_uses_per_order_fee_and_escrow() {
        let tokens = [Address::ZERO, Address::repeat_byte(1)];
        let amount = 10u128.pow(18);
        let b = Book {
            market: Address::repeat_byte(2),
            tokens,
            fees: [8, 2],
            dust: [U256::ZERO; 2],
            offers: [
                vec![
                    Offer {
                        id: U256::from(3256),
                        available: 100 * amount,
                        requested: 14800,
                    },
                    Offer {
                        id: U256::from(3255),
                        available: 30 * amount,
                        requested: 15300,
                    },
                ],
                vec![],
            ],
        };
        let full = b.plan(tokens[0], false, U256::from(130 * amount)).unwrap();
        assert!(full.complete);
        assert_eq!((full.spent, full.fees), (U256::from(30102), U256::from(2)));
        assert!(b.plan(tokens[1], true, U256::from(30102)).is_none());
        assert!(b.after(tokens[0], &full).unwrap().offers[0].is_empty());
        let smaller = b.plan(tokens[0], true, full.spent - U256::ONE).unwrap();
        assert!(smaller.spent < full.spent && smaller.received < full.received);
        assert_eq!(
            b.depth_in_band(tokens[0], true, 20, 100),
            Some(U256::from(14800))
        );
    }
}

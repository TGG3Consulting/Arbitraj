//! Antfarm single-hop ERC20 settlement. All fee fields are in raw ATF, independent of the output currency.
pub use crate::core::types::AntfarmMarket as Market;
use alloy_primitives::{Address, U256};

#[cfg(test)]
#[path = "antfarm_tests.rs"]
mod tests;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fill {
    pub parameter: U256,
    pub debit: U256,
    pub output: U256,
    pub fee_limit: U256,
    pub separate_atf: U256,
    pub next: Market,
}

impl Market {
    fn external_fee(&self, input: [U256; 2], output: [U256; 2]) -> Option<U256> {
        let (i, rate) = self.oracle?;
        let amount = input.get(i)?.checked_add(*output.get(i)?)?;
        let base = amount.checked_mul(U256::from(self.tier))? / U256::from(1000);
        let charge: U256 = rate.checked_mul(base)? >> 112;
        Some(charge)
    }

    /// Largest nominal router parameter whose *actual* debit fits the budget.
    pub fn parameter(&self, asset: Address, budget: U256, supporting: bool) -> Option<U256> {
        if asset != self.atf || supporting {
            return Some(budget);
        }
        let thousand = U256::from(1000);
        Some(
            budget
                .checked_mul(thousand)?
                .checked_add(thousand - U256::ONE)?
                / (thousand + U256::from(self.tier)),
        )
    }

    /// Standard-transfer model. Ingress/egress taxes require an independently measured wallet quote.
    /// No nominal quote can reuse dividend ATF or pay the third-token fee from the user's output.
    pub fn settle(&self, source: Address, budget: U256, supporting: bool) -> Option<Fill> {
        self.execute(source, budget, supporting, None)
    }

    /// Official single-hop swapTokensForExactTokens with ATF as target. Its inverse
    /// floors the gross ATF amount first, then adds one raw input even for an exact quotient.
    pub fn acquire_fee(&self, source: Address, amount: U256) -> Option<Fill> {
        if self.tokens != [self.atf, source] || self.tier >= 1000 || amount.is_zero() {
            return None;
        }
        let thousand = U256::from(1000);
        let raw = amount.checked_mul(thousand)? / (thousand - U256::from(self.tier));
        let remaining = self.reserves[0].checked_sub(raw)?;
        let cost = self.reserves[1]
            .checked_mul(raw)?
            .checked_div(remaining)?
            .checked_add(U256::ONE)?;
        self.execute(source, cost, false, Some(amount))
    }

    fn execute(
        &self,
        source: Address,
        budget: U256,
        supporting: bool,
        exact_atf: Option<U256>,
    ) -> Option<Fill> {
        let i = self.tokens.iter().position(|a| *a == source)?;
        let j = 1 - i;
        let thousand = U256::from(1000);
        let t = U256::from(self.tier);
        if t.is_zero()
            || t > thousand
            || (t == thousand && self.tokens[0] == self.atf)
            || self.lp_supply <= thousand
            || self.reserves.contains(&U256::ZERO)
        {
            return None;
        }
        let embedded = self.tokens[0] == self.atf;
        if self.tokens[1] == self.atf {
            return None;
        }
        let param = self.parameter(source, budget, supporting)?;
        let debit = if source == self.atf && !supporting {
            param.checked_mul(thousand + t)? / thousand
        } else {
            param
        };
        let mut balances = self.balances;
        balances[i] = balances[i].checked_add(debit)?;
        let priced = if supporting {
            balances[i].checked_sub(self.reserves[i])?
        } else {
            debit
        };
        let effective = if embedded && i == 0 {
            priced.checked_mul(thousand)? / (thousand + t)
        } else {
            priced
        };
        let nominal =
            self.reserves[j].checked_mul(effective)? / self.reserves[i].checked_add(effective)?;
        let out = if let Some(target) = exact_atf {
            target
        } else if embedded && j == 0 {
            nominal.checked_mul(thousand - t)? / thousand
        } else {
            nominal
        };
        if out.is_zero() || out >= self.reserves[j] {
            return None;
        }
        balances[j] = balances[j].checked_sub(out)?;
        let mut output = [U256::ZERO; 2];
        output[j] = out;
        let mut input = [U256::ZERO; 2];
        input[i] = priced;
        let limit = if embedded {
            if i == 0 {
                priced.checked_mul(t)? / (thousand + t)
            } else if supporting {
                priced.checked_mul(t)? / thousand
            } else {
                out.checked_mul(t)? / (thousand - t)
            }
        } else {
            self.external_fee(input, output)?
        };
        let mut reserves = [balances[0], balances[1]];
        if embedded {
            reserves[0] = reserves[0].checked_sub(self.dividends)?;
        }
        for k in 0..2 {
            input[k] = reserves[k].saturating_sub(self.reserves[k].checked_sub(output[k])?);
        }
        if input == [U256::ZERO; 2] {
            return None;
        }
        let physical = if embedded {
            (input[0].checked_mul(t)? / (thousand + t))
                .checked_add(output[0].checked_mul(t)? / (thousand - t))?
        } else {
            self.external_fee(input, output)?
        };
        if physical < thousand {
            return None;
        }
        if embedded {
            reserves[0] = reserves[0].checked_sub(physical)?;
        } else {
            balances[2] = balances[2].checked_add(limit)?;
            if balances[2].checked_sub(self.dividends)? < physical {
                return None;
            }
        }
        if reserves.iter().any(|v| v.bit_len() > 112)
            || reserves[0].checked_mul(reserves[1])?
                < self.reserves[0].checked_mul(self.reserves[1])?
        {
            return None;
        }
        let new_dividends = physical.checked_mul(U256::from(8500))? / U256::from(10000);
        let burned = physical.checked_sub(new_dividends)?;
        if embedded {
            balances[0] = balances[0].checked_sub(burned)?;
            balances[2] = balances[0];
        } else {
            balances[2] = balances[2].checked_sub(burned)?;
        }
        let mut next = self.clone();
        next.dividends = self.dividends.checked_add(new_dividends)?;
        next.balances = balances;
        next.reserves = reserves;
        Some(Fill {
            parameter: param,
            debit,
            output: out,
            fee_limit: limit,
            separate_atf: if embedded { U256::ZERO } else { limit },
            next,
        })
    }
}

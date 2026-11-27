//! Антфарм: расчёт в валюте выплаты, включая восполнение ATF и плату кредитору.
use super::{antfarm::Fill, full_math::mul_div_up};
use crate::core::types::AntfarmBook;
use alloy_primitives::{Address, U256};

pub use crate::core::types::AntfarmFunds as Funds;
#[derive(Debug, Clone)]
pub struct OutputFill {
    pub primary: Fill,
    pub refill: Fill,
    pub loan_fee: U256,
    pub credit: U256,
}
impl AntfarmBook {
    pub fn paid_from_output(
        &self,
        input: Address,
        amount: U256,
        tier: u16,
        funds: Funds,
    ) -> Option<OutputFill> {
        self.terms_from_output(input, amount, tier, funds)
            .filter(|v| !v.credit.is_zero())
    }
    /// Invert the *net* amount due to the payer. Every iterate is a lower bound:
    /// any larger primary swap must buy at least this much ATF from the same funding curve.
    pub fn output_order(&self, input: Address, exact_in: bool, size: U256) -> Option<OutputFill> {
        let (tier, funds) = self.proceeds?;
        self.output_at(input, exact_in, size, tier, funds)
    }
    pub(super) fn output_at(
        &self,
        input: Address,
        exact_in: bool,
        size: U256,
        tier: u16,
        funds: Funds,
    ) -> Option<OutputFill> {
        if exact_in {
            return self.paid_from_output(input, size, tier, funds);
        }
        let i = self.market.tokens.iter().position(|v| *v == input)?;
        let mut gross = size;
        for _ in 0..128 {
            let amount = self.market.minimum_for(i, gross)?;
            let f = self.terms_from_output(input, amount, tier, funds)?;
            if f.credit >= size {
                return Some(f);
            }
            let required = size.checked_add(f.refill.debit)?.checked_add(f.loan_fee)?;
            if required <= gross {
                return None;
            }
            gross = required;
        }
        None
    }
    /// Calculate one real pair of curves. The caller compares all viable tiers after gas.
    fn terms_from_output(
        &self,
        input: Address,
        amount: U256,
        funding_tier: u16,
        funds: Funds,
    ) -> Option<OutputFill> {
        let primary = &self.market;
        let i = primary.tokens.iter().position(|v| *v == input)?;
        let output = primary.tokens[1 - i];
        if primary.tokens.contains(&primary.atf) || amount.is_zero() {
            return None;
        }
        let (refill, projected) = self
            .funding
            .iter()
            .find(|(m, _)| m.tokens == [primary.atf, output] && m.tier == funding_tier)?;
        let mut state = primary.clone();
        // A cash loan buys ATF first. With the owner's ATF the primary settles before the oracle can update.
        if matches!(funds, Funds::Vault { .. })
            && let Some((oi, rate)) = state.oracle.as_mut()
            && *oi == 1 - i
            && funding_tier == 10
        {
            *rate = (*projected)?;
        }
        let paid = state.settle(input, amount, false)?;
        if paid.debit != amount {
            return None;
        }
        let replenished = refill.acquire_fee(output, paid.separate_atf)?;
        let premium = match funds {
            Funds::Wallet { atf } if atf >= paid.separate_atf => U256::ZERO,
            Funds::Vault {
                output_balance,
                fee_per_1e18,
            } if output_balance >= replenished.debit => mul_div_up(
                replenished.debit,
                fee_per_1e18,
                U256::from(1_000_000_000_000_000_000u64),
            )?,
            _ => return None,
        };
        let credit = paid
            .output
            .saturating_sub(replenished.debit)
            .saturating_sub(premium);
        Some(OutputFill {
            primary: paid,
            refill: replenished,
            loan_fee: premium,
            credit,
        })
    }
}

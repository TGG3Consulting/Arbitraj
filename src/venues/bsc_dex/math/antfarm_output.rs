//! Антфарм: расчёт в валюте выплаты, включая восполнение ATF и плату кредитору.
use super::{
    antfarm::{Fill, Market},
    full_math::mul_div_up,
};
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
struct Payment<'a> {
    primary: Market,
    refill: &'a Market,
    side: usize,
    funds: Funds,
}

impl AntfarmBook {
    fn payment(&self, input: Address, tier: u16, funds: Funds) -> Option<Payment<'_>> {
        let mut primary = self.market.clone();
        let side = primary.tokens.iter().position(|a| *a == input)?;
        if primary.tokens.contains(&primary.atf) {
            return None;
        }
        let output = primary.tokens[1 - side];
        let (refill, projected) = self
            .funding
            .iter()
            .find(|(m, _)| m.tokens == [primary.atf, output] && m.tier == tier)?;
        // The vault purchases ATF before the primary swap, while stocked ATF is replenished afterwards.
        if matches!(funds, Funds::Vault { .. })
            && tier == 10
            && let Some((priced, rate)) = primary.oracle.as_mut()
            && *priced == 1 - side
        {
            *rate = (*projected)?;
        }
        Some(Payment {
            primary,
            refill,
            side,
            funds,
        })
    }
    pub fn paid_from_output(
        &self,
        input: Address,
        amount: U256,
        tier: u16,
        funds: Funds,
    ) -> Option<OutputFill> {
        self.payment(input, tier, funds)?
            .fill(amount)
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
        if size.is_zero() {
            return None;
        }
        let payment = self.payment(input, tier, funds)?;
        let minimum = payment.minimum_debit()?;
        let mut gross = size;
        for _ in 0..128 {
            let amount = minimum.max(payment.primary.minimum_for(payment.side, gross)?);
            let f = payment.fill(amount)?;
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
}
impl Payment<'_> {
    /// Both pools reject a physical ATF fee below 1,000 raw units. Donations can pay part of either floor.
    /// Translate each necessary fee back into the primary oracle's native input or output units.
    fn minimum_debit(&self) -> Option<U256> {
        let thousand = U256::from(1000);
        let (priced, rate) = self.primary.oracle?;
        let ft = U256::from(self.refill.tier);
        let settled_atf = self.refill.balances[0].checked_sub(self.refill.dividends)?;
        let donated = settled_atf.saturating_sub(self.refill.reserves[0]);
        let prepaid = donated.checked_mul(ft)? / (thousand + ft);
        let refill_floor = mul_div_up(
            thousand.saturating_sub(prepaid),
            thousand.checked_sub(ft)?,
            ft,
        )?
        .max(U256::ONE);
        let priced_units = |fee| {
            let basis = mul_div_up(fee, U256::ONE << 112, rate)?;
            mul_div_up(basis, thousand, U256::from(self.primary.tier))
        };
        let extra = self.primary.balances[priced].saturating_sub(self.primary.reserves[priced]);
        let threshold =
            priced_units(refill_floor)?.max(priced_units(thousand)?.saturating_sub(extra));
        if priced == self.side {
            Some(threshold)
        } else {
            self.primary.minimum_for(self.side, threshold)
        }
    }
    fn fill(&self, amount: U256) -> Option<OutputFill> {
        let input = self.primary.tokens[self.side];
        let output = self.primary.tokens[1 - self.side];
        let paid = self.primary.settle(input, amount, false)?;
        if paid.debit != amount {
            return None;
        }
        let replenished = self.refill.acquire_fee(output, paid.separate_atf)?;
        let premium = match self.funds {
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

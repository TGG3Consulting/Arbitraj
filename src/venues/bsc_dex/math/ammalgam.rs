//! Exact integer branches of AmmalgamPair.calculateBalanceAfterFees and QuadraticSwapFees.
//! Source of the beacon implementation on Ethereum at 26494988:
//! https://repo.sourcify.dev/1/0x5C0d819609Eee2cdCf326f0e898A8447e06a45Be
use super::swap_cl::Swapped;
use crate::core::types::AmmalgamState;
use alloy_primitives::U256;

const Q64: u128 = 1 << 64;
const MIN_FEE: u128 = 0x1999999999999999;
const BIPS: u128 = 10_000;
const MAX_QUADRATIC_BIPS: u128 = 4_000;
const BUFFER_NUMERATOR: u8 = 20;

fn fee_bips(amount: U256, reserve: U256, reference: U256) -> Option<U256> {
    if reference.is_zero() || amount.is_zero() {
        return None;
    }
    let base = reserve.max(reference);
    let past = amount.checked_add(reserve)?.saturating_sub(base);
    let deviation = past.checked_add((base - reference).checked_mul(U256::from(2))?)?;
    let slope = U256::from(MAX_QUADRATIC_BIPS * Q64);
    let fee = if deviation <= reference {
        slope.checked_mul(deviation)? / reference
    } else {
        let tail = reference.checked_mul(U256::from(Q64))? / deviation;
        U256::from(MAX_QUADRATIC_BIPS).checked_mul(U256::from(2*Q64) - tail)?
    };
    Some((fee.checked_mul(past)? / amount).max(U256::from(MIN_FEE)))
}

// Convert.calculateReserveAdjustmentsForMissingAssets is the lesser of the two branches.
fn adjusted(reserve: U256, missing: U256) -> Option<U256> {
    Some(reserve.min(reserve.checked_sub(missing)?.checked_mul(U256::from(BUFFER_NUMERATOR))?))
}

impl AmmalgamState {
    fn payout(&self, i: usize, amount: U256) -> Option<U256> {
        let o = 1 - i;
        let r = self.reserves;
        if amount.is_zero() { return Some(U256::ZERO); }
        let fee = amount.checked_mul(fee_bips(amount,r[i],self.reference[i])?)?.div_ceil(U256::from(BIPS*Q64));
        let after_in = adjusted(r[i].checked_add(amount.checked_sub(fee)?)?,self.missing[i])?;
        if after_in.is_zero() { return None; }
        let invariant = adjusted(r[i],self.missing[i])?.checked_mul(adjusted(r[o],self.missing[o])?)?;
        let scaled = invariant.div_ceil(after_in);
        let remaining = scaled.max(self.missing[o].checked_add(scaled.div_ceil(U256::from(BUFFER_NUMERATOR)))?);
        r[o].checked_sub(remaining)
    }

    pub(crate) fn swap(&self, zero_in: bool, exact_in: bool, amount: U256) -> Option<Swapped> {
        let i=usize::from(!zero_in);
        if amount.is_zero() {return None;}
        // The original pair stores its updated reserves in uint112.
        let max=(U256::from(u128::MAX >> 16)).checked_sub(self.reserves[i])?;
        let input = if exact_in {
            amount
        } else {
            if amount > self.cash[1-i] || self.payout(i,max)? < amount {return None;}
            let (mut low,mut high)=(U256::ONE,max);
            while low<high {
                let mid=low+(high-low)/U256::from(2);
                if self.payout(i,mid)? >= amount {high=mid;} else {low=mid+U256::ONE;}
            }
            low
        };
        if input > max {return None;}
        let gross=self.payout(i,input)?;
        if gross.is_zero() || gross > self.cash[1-i] || gross >= self.reserves[1-i].checked_sub(self.missing[1-i])? {return None;}
        Some(Swapped{amount_in:input,amount_out:if exact_in {gross} else {amount},complete:true})
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    #[test]
    fn integer_cash_curve_matches_the_verified_helper_on_all_ten_mainnet_markets() {
        let mut tape=Vec::new();
        flate2::read::GzDecoder::new(include_bytes!("../../../../tests/data/first_run_dex_2026-11-26/ammalgam_ethereum_quotes.json.gz").as_slice()).read_to_end(&mut tape).unwrap();
        let data:serde_json::Value=serde_json::from_slice(&tape).unwrap();
        let number=|v:&serde_json::Value|v.as_str().unwrap().parse::<U256>().unwrap();
        let mut mismatches=Vec::new();
        for row in data["rows"].as_array().unwrap() {
            let s=&row["state"];
            let market=AmmalgamState{reserves:[number(&s[0]),number(&s[2])],reference:[number(&s[1]),U256::ZERO],
                missing:[number(&s[3]),number(&s[4])],cash:[U256::ZERO,number(&row["output_cash"])]};
            for quote in row["quotes"].as_array().unwrap() {
                let amount=number(&quote["amount"]);
                let actual=market.payout(0,amount);
                let expected=number(&quote["quote"]);
                if actual!=Some(expected) {
                    mismatches.push((row["pool"].as_str().unwrap(),amount,actual,expected));
                }
            }
        }
        assert!(mismatches.is_empty(),"{} quotes differ; first three: {:?}",mismatches.len(),&mismatches[..mismatches.len().min(3)]);
    }
}

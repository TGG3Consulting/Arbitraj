//! https://docs.motoswap.org/dev/contracts — LP fee in the pair and independent protocol/creator floor divisions in FeeRouter.
use super::{full_math, swap_cl::Swapped, v2};
use crate::core::types::MotoswapState;
use alloy_primitives::U256;

const D: u16 = 10_000;

impl MotoswapState {
    pub(crate) fn net(&self, amount: U256) -> Option<U256> {
        let cut = |bps: u16| full_math::mul_div(amount, U256::from(bps), U256::from(D));
        amount
            .checked_sub(cut(self.protocol_bps)?)?
            .checked_sub(cut(self.creator_bps)?.checked_mul(U256::from(self.creators))?)
    }

    /// The real-valued net is g*(D-fees)/D. Rounding can add strictly less than
    /// one atom per nonzero cut. Scan only this short ambiguity interval:
    /// simultaneous fee increments can decrease net by an atom.
    fn fee_interval(&self, want: U256) -> Option<(U256, U256)> {
        let n = u16::from(self.protocol_bps > 0)
            + u16::from(self.creator_bps > 0) * u16::from(self.creators);
        if n == 0 {
            return Some((want, want));
        }
        let rate = D
            .checked_sub(self.protocol_bps)?
            .checked_sub(self.creator_bps.checked_mul(u16::from(self.creators))?)?;
        if rate == 0 {
            return None;
        }
        let lo = full_math::mul_div(
            want.saturating_sub(U256::from(n)),
            U256::from(D),
            U256::from(rate),
        )?
        .checked_add(U256::ONE)?
        .max(want);
        let hi = full_math::mul_div_up(want, U256::from(D), U256::from(rate))?;
        Some((lo, hi))
    }

    fn forward(&self, i: usize, n: U256) -> Option<Swapped> {
        let j = 1 - i;
        let r = self.reserves;
        let fee_out = self.fee_on_output[i];
        let lp = U256::from(D.checked_sub(self.lp_bps)?);
        let unit = U256::from(D);
        let landed = if fee_out { n } else { self.net(n)? };
        let x = self.balances[i].checked_add(landed)?;
        let input = x.checked_sub(r[i])?;
        let gross = v2::amount_out(input, r[i], r[j], (lp, unit))?;
        let y = self.balances[j].checked_sub(gross)?;
        let max = U256::from(u128::MAX >> 16);
        if gross.is_zero() || x > max || y > max {
            return None;
        }
        let amount_out = if fee_out { self.net(gross)? } else { gross };
        if amount_out.is_zero() {
            return None;
        }
        if self.balances[j] != r[j] {
            let other_in = self.balances[j].saturating_sub(r[j]);
            let adjust = |balance: U256, credit: U256| {
                balance
                    .checked_mul(unit)?
                    .checked_sub(credit.checked_mul(U256::from(self.lp_bps))?)
            };
            if adjust(x, input)?.checked_mul(adjust(y, other_in)?)?
                < r[i].checked_mul(r[j])?.checked_mul(unit * unit)?
            {
                return None;
            }
        }
        Some(Swapped {
            amount_in: n,
            amount_out,
            complete: true,
        })
    }

    fn input_for_gross(&self, i: usize, gross: U256) -> Option<U256> {
        let r = self.reserves;
        let input = full_math::mul_div_up(
            r[i].checked_mul(U256::from(D))?,
            gross,
            r[1 - i]
                .checked_sub(gross)?
                .checked_mul(U256::from(D.checked_sub(self.lp_bps)?))?,
        )?;
        Some(
            r[i].checked_add(input)?
                .saturating_sub(self.balances[i])
                .max(U256::ONE),
        )
    }

    pub(crate) fn swap(&self, zero_for_one: bool, exact_in: bool, amount: U256) -> Option<Swapped> {
        if amount.is_zero()
            || self.creators > 2
            || self.lp_bps > 200
            || self.protocol_bps > 100
            || self.creator_bps > 50
        {
            return None;
        }
        let i = usize::from(!zero_for_one);
        if exact_in {
            return self.forward(i, amount);
        }
        let out_fee = self.fee_on_output[i];
        let target = if out_fee {
            amount
        } else {
            self.input_for_gross(i, amount)?
        };
        let (mut g, end) = self.fee_interval(target)?;
        while g <= end {
            let n = if out_fee {
                self.input_for_gross(i, g)?
            } else {
                g
            };
            if let Some(fill) = self.forward(i, n)
                && fill.amount_out >= amount
            {
                return Some(fill);
            }
            g = g.checked_add(U256::ONE)?;
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    #[test]
    fn both_directions_use_actual_post_purchase_reserves_and_creator_floors() {
        let archive = include_bytes!(
            "../../../../tests/data/first_run_dex_2026-11-26/motoswap_ethereum.json.gz"
        );
        let mut body = String::new();
        flate2::read::GzDecoder::new(&archive[..])
            .read_to_string(&mut body)
            .unwrap();
        let data: serde_json::Value = serde_json::from_str(&body).unwrap();
        let book = &data["directory"];
        let mut tested = 0;
        for legs in data["trials"].as_array().unwrap().chunks_exact(2) {
            let pool = book["pairs"]
                .as_array()
                .unwrap()
                .iter()
                .find(|p| p["pool"] == legs[0]["pool"])
                .unwrap();
            let a = pool["tokens"][0].as_str().unwrap();
            let b = pool["tokens"][1].as_str().unwrap();
            let num = |n: &serde_json::Value| n.as_str().unwrap().parse::<U256>().unwrap();
            let reserves = [num(&pool["reserves"][0]), num(&pool["reserves"][1])];
            let mut state = MotoswapState {
                sides: None,
                path_module:None,
                fee_router_frozen: false,
                reserves,
                balances: reserves,
                lp_bps: 30,
                protocol_bps: 70,
                creator_bps: 30,
                creators: [a, b]
                    .iter()
                    .filter(|t| {
                        book["tokens"][**t]["creator"].as_str().unwrap()
                            != "0x0000000000000000000000000000000000000000"
                    })
                    .count() as u8,
                fee_on_output: [
                    pool["fee_on_output"][0].as_bool().unwrap(),
                    pool["fee_on_output"][1].as_bool().unwrap(),
                ],
            };
            for leg in legs {
                let pay = &leg["paid"];
                let zero = pay["input"].as_str().unwrap() == a;
                let qty = num(&pay["spent"]);
                let net = num(&pay["received"]);
                assert_eq!(state.swap(zero, true, qty).unwrap().amount_out, net);
                let least = state.swap(zero, false, net).unwrap();
                assert!(least.amount_in <= qty && least.amount_out >= net);
                assert!(
                    state
                        .swap(zero, true, least.amount_in - U256::ONE)
                        .is_none_or(|f| f.amount_out < net)
                );
                let i = usize::from(!zero);
                state.reserves[i] += num(&pay["pool_delta"][0]);
                state.reserves[1 - i] -= pay["pool_delta"][1]
                    .as_str()
                    .unwrap()
                    .strip_prefix('-')
                    .unwrap()
                    .parse::<U256>()
                    .unwrap();
                state.balances = state.reserves;
                tested += 1;
            }
        }
        assert_eq!(tested, 16);
    }
}

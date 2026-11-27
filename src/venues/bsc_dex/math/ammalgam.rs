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
        U256::from(MAX_QUADRATIC_BIPS).checked_mul(U256::from(2 * Q64) - tail)?
    };
    Some((fee.checked_mul(past)? / amount).max(U256::from(MIN_FEE)))
}

// Convert.calculateReserveAdjustmentsForMissingAssets is the lesser of the two branches.
fn adjusted(reserve: U256, missing: U256) -> Option<U256> {
    Some(
        reserve.min(
            reserve
                .checked_sub(missing)?
                .checked_mul(U256::from(BUFFER_NUMERATOR))?,
        ),
    )
}

impl AmmalgamState {
    /// Adjusted reserves are piecewise linear. As input grows the input slope may
    /// fall from 20 to 1; as output leaves the output slope may rise from 1 to 20.
    /// The right-hand marginal output/input rate is therefore an upper bound.
    pub(crate) fn marginal(&self, i: usize) -> Option<(U256, U256, U256, U256)> {
        let o = 1 - i;
        let x = adjusted(self.reserves[i], self.missing[i])?;
        let y = adjusted(self.reserves[o], self.missing[o])?;
        if x.is_zero() || y.is_zero() {
            return None;
        }
        let buffer = U256::from(BUFFER_NUMERATOR);
        let slope_in = if x < self.reserves[i] {
            buffer
        } else {
            U256::ONE
        };
        let slope_out =
            if buffer.checked_mul(self.reserves[o] - self.missing[o])? <= self.reserves[o] {
                buffer
            } else {
                U256::ONE
            };
        Some((x, y, slope_in, slope_out))
    }

    /// Depth without the swap fee, measured to the marginal price boundary.
    /// On each of the four linear pieces, price/initial_price equals
    /// (initial_x/current_x)^2 * (input_slope/initial_input_slope) *
    /// (initial_output_slope/output_slope). Clip at each discontinuity and at cash.
    pub(crate) fn band(
        &self,
        i: usize,
        buy: bool,
        fraction: U256,
        unit: U256,
    ) -> Option<(U256, bool)> {
        if fraction.is_zero() || fraction >= unit {
            return None;
        }
        let o = 1 - i;
        let Some((x, y, initial_in, initial_out)) = self.marginal(i) else {
            return Some((U256::ZERO, false));
        };
        let (numerator, denominator) = if buy {
            (unit + fraction, unit)
        } else {
            (unit, unit - fraction)
        };
        let k = x.checked_mul(y)?;
        let buffer = U256::from(BUFFER_NUMERATOR);
        let one = U256::ONE;
        let raw_limit = U256::from(u128::MAX >> 16);
        let mut last = self.reserves[i];
        for slope_in in [one, buffer] {
            for slope_out in [one, buffer] {
                let n = numerator.checked_mul(slope_in)?.checked_mul(initial_out)?;
                let d = denominator
                    .checked_mul(slope_out)?
                    .checked_mul(initial_in)?;
                let mut scaled = super::full_math::mul_div(x.checked_mul(x)?, n, d)
                    .unwrap_or(U256::MAX)
                    .root(2);
                // Keep each candidate inside the region where its assumed slope is
                // attainable. A more conservative slope may cross an earlier piece.
                if slope_in == buffer {
                    scaled = scaled.min(self.missing[i].checked_mul(buffer)? / (buffer - one));
                }
                if slope_out == one && !self.missing[o].is_zero() {
                    scaled = scaled.min(super::full_math::mul_div(
                        k,
                        buffer - one,
                        self.missing[o].checked_mul(buffer)?,
                    )?);
                }
                let reserve = scaled.max(self.missing[i].checked_add(scaled / buffer)?);
                last = last.max(reserve);
            }
        }
        let saturated_reserve = last.min(raw_limit);
        let physical = self.reserves[o]
            .saturating_sub(self.missing[o])
            .saturating_sub(one)
            .min(self.cash[o]);
        if physical.is_zero() {
            return Some((U256::ZERO, false));
        }
        let remaining = Self::reserve_for(
            k.div_ceil(adjusted(saturated_reserve, self.missing[i])?),
            self.missing[o],
        )?;
        let out = self.reserves[o].checked_sub(remaining)?;
        if out >= physical {
            let needed = Self::reserve_for(
                k.div_ceil(adjusted(self.reserves[o] - physical, self.missing[o])?),
                self.missing[i],
            )?;
            return Some((
                if buy {
                    needed.checked_sub(self.reserves[i])?
                } else {
                    physical
                },
                false,
            ));
        }
        Some((
            if out.is_zero() {
                U256::ZERO
            } else if buy {
                saturated_reserve - self.reserves[i]
            } else {
                out
            },
            last <= raw_limit,
        ))
    }

    fn payout(&self, i: usize, amount: U256) -> Option<U256> {
        let o = 1 - i;
        let r = self.reserves;
        if amount.is_zero() {
            return Some(U256::ZERO);
        }
        let fee = amount
            .checked_mul(fee_bips(amount, r[i], self.reference[i])?)?
            .div_ceil(U256::from(BIPS * Q64));
        let after_in = adjusted(r[i].checked_add(amount.checked_sub(fee)?)?, self.missing[i])?;
        if after_in.is_zero() {
            return None;
        }
        let invariant =
            adjusted(r[i], self.missing[i])?.checked_mul(adjusted(r[o], self.missing[o])?)?;
        let scaled = invariant.div_ceil(after_in);
        let remaining = Self::reserve_for(scaled, self.missing[o])?;
        r[o].checked_sub(remaining)
    }

    /// A reserve must satisfy both straight branches of the adjusted-reserve function.
    fn reserve_for(adjusted: U256, missing: U256) -> Option<U256> {
        Some(adjusted.max(missing.checked_add(adjusted.div_ceil(U256::from(BUFFER_NUMERATOR)))?))
    }
    fn input_for(&self, i: usize, wanted: U256, max: U256) -> Option<U256> {
        let o = 1 - i;
        let cash_out = self.reserves[o].checked_sub(self.missing[o])?;
        if wanted >= cash_out || wanted > self.cash[o] {
            return None;
        }
        let right = adjusted(self.reserves[o] - wanted, self.missing[o])?;
        if right.is_zero() {
            return None;
        }
        let k = adjusted(self.reserves[i], self.missing[i])?
            .checked_mul(adjusted(self.reserves[o], self.missing[o])?)?;
        let net =
            Self::reserve_for(k.div_ceil(right), self.missing[i])?.checked_sub(self.reserves[i])?;
        if net.is_zero() {
            return None;
        }
        let unit = U256::from(BIPS * Q64);
        let numerator = net.checked_mul(unit)?;
        let mut gross = net;
        // fee_bips never decreases. At this candidate the minimum gross covering net at its
        // current rate is a lower bound for every feasible input. Jump directly to that bound.
        // Equal bounds are an executable fixed point, including integer rounding on the fee.
        while gross <= max {
            let rate = fee_bips(gross, self.reserves[i], self.reference[i])?;
            let next = numerator.div_ceil(unit.checked_sub(rate)?);
            if next <= gross {
                return Some(gross);
            }
            gross = next;
        }
        None
    }

    pub(crate) fn swap(&self, zero_in: bool, exact_in: bool, amount: U256) -> Option<Swapped> {
        let i = usize::from(!zero_in);
        if amount.is_zero() || self.reserves[i] <= self.missing[i] {
            return None;
        }
        // The original pair stores its updated reserves in uint112.
        let max = (U256::from(u128::MAX >> 16)).checked_sub(self.reserves[i])?;
        let input = if exact_in {
            amount
        } else {
            self.input_for(i, amount, max)?
        };
        if input > max {
            return None;
        }
        // Like the pair's swap, the protected wallet limits its request to actual
        // output inventory and leaves at least one raw unit above uncovered borrows.
        let pair_limit = self.reserves[1 - i]
            .checked_sub(self.missing[1 - i])?
            .checked_sub(U256::ONE)?;
        let gross = self.payout(i, input)?.min(pair_limit).min(self.cash[1 - i]);
        if gross.is_zero() || (!exact_in && gross < amount) {
            return None;
        }
        Some(Swapped {
            amount_in: input,
            amount_out: if exact_in { gross } else { amount },
            complete: true,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    #[test]
    fn depth_matches_an_atom_sweep_across_both_piecewise_reserve_boundaries() {
        let w = U256::from;
        let buffer = 20_u128;
        let adjusted = |r: u128, m: u128| r.min(buffer * (r - m));
        for (rin, rout) in [(20_u128, 40_u128), (40, 20), (55, 76)] {
            for mi in [0, rin * 19 / 20, rin - 1] {
                for mo in [0, rout * 19 / 20, rout - 1] {
                    let x = adjusted(rin, mi);
                    let y = adjusted(rout, mo);
                    let k = x * y;
                    let si = if x < rin { buffer } else { 1 };
                    let so = if (rout - mo) * buffer <= rout {
                        buffer
                    } else {
                        1
                    };
                    for cash in [1, rout / 2, rout] {
                        let cap = cash.min(rout - mo - 1);
                        let state = AmmalgamState {
                            reserves: [w(rin), w(rout)],
                            reference: [w(rin), w(rout)],
                            missing: [w(mi), w(mo)],
                            cash: [w(rin), w(cash)],
                        };
                        for pct in [1, 20, 75, 90] {
                            for buy in [true, false] {
                                let (top, bottom) = if buy {
                                    (100 + pct, 100)
                                } else {
                                    (100, 100 - pct)
                                };
                                let mut depth = 0;
                                for dx in 1..=800 {
                                    let rx = rin + dx;
                                    let ax = adjusted(rx, mi);
                                    // Left-hand derivative at the terminal point: the whole
                                    // continuous interval up to it must stay within the band.
                                    let sx = if buffer * (rx - mi) <= rx { buffer } else { 1 };
                                    let sy = if k * (buffer - 1) < ax * buffer * mo {
                                        buffer
                                    } else {
                                        1
                                    };
                                    if x * x * sx * so * top < ax * ax * sy * si * bottom {
                                        break;
                                    }
                                    let ay = k.div_ceil(ax);
                                    let output = rout - ay.max(mo + ay.div_ceil(buffer));
                                    if output > 0 && cap > 0 {
                                        depth = if buy { dx } else { output.min(cap) };
                                    }
                                    if output >= cap {
                                        break;
                                    }
                                }
                                let actual = state.band(0, buy, w(pct), w(100)).unwrap().0;
                                assert_eq!(
                                    actual,
                                    w(depth),
                                    "{rin}/{rout}, debt {mi}/{mo}, cash {cash}, band {pct}, buy {buy}"
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn rounded_fee_can_lower_the_payout_when_input_increases_by_one() {
        // Both calls recorded from computeExpectedSwapAmountOut on Ethereum. The reserve
        // arrangement fits the deployed ABI and deliberately lands on a Q64 fee step.
        let reserve = U256::ONE << 110;
        let state = AmmalgamState {
            reserves: [reserve; 2],
            reference: [reserve; 2],
            missing: [U256::ZERO; 2],
            cash: [reserve; 2],
        };
        let breakpoint = U256::ONE << 109;
        let target = U256::from(370878347038201973466465818636745u128);
        assert_eq!(state.payout(0, breakpoint - U256::ONE), Some(target));
        assert_eq!(
            state.payout(0, breakpoint),
            Some(U256::from(370878347038201973466464023515720u128))
        );
        let minimum = state.swap(true, false, target).unwrap().amount_in;
        assert!(minimum < breakpoint && state.payout(0, minimum).unwrap() >= target);
        assert!(state.payout(0, minimum - U256::ONE).unwrap() < target);
    }

    #[test]
    fn integer_cash_curve_matches_the_verified_helper_on_all_ten_mainnet_markets() {
        let mut tape = Vec::new();
        flate2::read::GzDecoder::new(
            include_bytes!(
                "../../../../tests/data/first_run_dex_2026-11-26/ammalgam_ethereum_quotes.json.gz"
            )
            .as_slice(),
        )
        .read_to_end(&mut tape)
        .unwrap();
        let data: serde_json::Value = serde_json::from_slice(&tape).unwrap();
        let number = |v: &serde_json::Value| v.as_str().unwrap().parse::<U256>().unwrap();
        let mut mismatches = Vec::new();
        let (mut equal, mut over, mut short) = (0, 0, 0);
        for row in data["rows"].as_array().unwrap() {
            let s = &row["state"];
            let market = AmmalgamState {
                reserves: [number(&s[0]), number(&s[2])],
                reference: [number(&s[1]), U256::ZERO],
                missing: [number(&s[3]), number(&s[4])],
                cash: [U256::ZERO, number(&row["output_cash"])],
            };
            for saved in row["inverse"].as_array().unwrap() {
                let target = number(&saved["target"]);
                let suggestion = number(&saved["suggested_input"]);
                let fill = market
                    .swap(true, false, target)
                    .expect("funded reverse quote");
                let paid = fill.amount_in;
                assert!(
                    market.payout(0, paid).unwrap() >= target
                        && market.payout(0, paid - U256::ONE).unwrap() < target
                );
                match suggestion.cmp(&paid) {
                    std::cmp::Ordering::Equal => equal += 1,
                    std::cmp::Ordering::Greater => {
                        over += 1;
                    }
                    std::cmp::Ordering::Less => {
                        assert!(market.payout(0, suggestion).unwrap() < target);
                        short += 1;
                    }
                }
            }
            for quote in row["quotes"].as_array().unwrap() {
                let amount = number(&quote["amount"]);
                let actual = market.payout(0, amount);
                let expected = number(&quote["quote"]);
                if actual != Some(expected) {
                    mismatches.push((row["pool"].as_str().unwrap(), amount, actual, expected));
                }
            }
        }
        assert_eq!(equal + over + short, 135);
        if let Some(path) = std::env::var_os("ARB_AMMALGAM_INVERSE_EXPORT") {
            let result = serde_json::json!({"block":26494988,"forward":185,"inverse":equal+over+short,
                "minimum_matches_helper":equal,"helper_input_above_minimum":over,"helper_input_below_minimum":short});
            std::fs::write(path, serde_json::to_vec(&result).unwrap()).unwrap();
        }
        assert!(
            mismatches.is_empty(),
            "{} quotes differ; first three: {:?}",
            mismatches.len(),
            &mismatches[..mismatches.len().min(3)]
        );
    }
}

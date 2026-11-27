//! Предельная цена и глубина пула Ammalgam с учётом доступного остатка.
use super::{AmmalgamState, BUFFER_NUMERATOR, U256, adjusted};

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
}

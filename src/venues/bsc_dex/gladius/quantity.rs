//! Количество покупаемого у maker токена при точном дебете продавца. Исходники
//! GladiusReactor.resolve, DutchDecayLib.decay и PartialFillLib.partition из подтверждённого Reactor.
use super::{abi::Order, *};
use crate::venues::bsc_dex::math::full_math::mul_div;
pub(super) struct Curve {
    pub(super) quantity: U256,
    output: U256,
}
impl Curve {
    pub(super) fn at(order: &Order, stamp: u64) -> Option<Self> {
        let now = U256::from(stamp);
        let amount = |start: U256, end: U256| -> Option<U256> {
            if now >= order.end {
                return Some(end);
            }
            if now <= order.start {
                return Some(start);
            }
            let elapsed = now - order.start;
            let duration = order.end - order.start;
            if end >= start {
                start.checked_add(mul_div(end - start, elapsed, duration)?)
            } else {
                start.checked_sub(mul_div(start - end, elapsed, duration)?)
            }
        };
        let output = order.outputs.first()?;
        if output.endAmount > output.startAmount {
            return None;
        }
        Some(Self {
            quantity: amount(order.input.startAmount, order.input.endAmount)?,
            output: amount(output.startAmount, output.endAmount)?,
        })
    }
    // Плата за исключительное исполнение и протокольная комиссия добавляются к
    // ceil(q * output / quantity). Среди равных номинальных выплат выгодно взять максимум q.
    pub(super) fn sell(&self, wanted: U256, minimum: U256, cap: U256) -> Search<'_> {
        let hi = wanted.min(self.output);
        Search {
            curve: self,
            wanted,
            minimum: minimum.max(U256::ONE),
            cap: cap.min(self.quantity),
            lo: U256::ONE,
            hi,
            nominal: hi,
            interpolate: true,
        }
    }
}
pub(super) struct Search<'a> {
    curve: &'a Curve,
    wanted: U256,
    pub(super) minimum: U256,
    cap: U256,
    lo: U256,
    hi: U256,
    nominal: U256,
    interpolate: bool,
}
impl Search<'_> {
    pub(super) fn quantity(&self) -> Option<U256> {
        if self.lo > self.hi {
            return None;
        }
        Some(mul_div(self.nominal, self.curve.quantity, self.curve.output)?.min(self.cap))
    }
    pub(super) fn advance(&mut self, q: U256, debit: U256) -> Option<()> {
        if debit > self.wanted {
            self.hi = self.nominal.checked_sub(U256::ONE)?;
        } else {
            if q == self.cap {
                return None;
            }
            self.lo = self.nominal.checked_add(U256::ONE)?;
        }
        if self.lo > self.hi {
            return None;
        }
        let mid = self.lo + (self.hi - self.lo) / U256::from(2);
        self.nominal = if self.interpolate {
            mul_div(self.nominal, self.wanted, debit)
                .filter(|n| *n >= self.lo && *n <= self.hi)
                .unwrap_or(mid)
        } else {
            mid
        };
        self.interpolate = !self.interpolate;
        Some(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn find(
        curve: &Curve,
        wanted: U256,
        min: U256,
        cap: U256,
        quote: impl Fn(U256) -> Option<U256>,
    ) -> Option<U256> {
        let mut s = curve.sell(wanted, min, cap);
        loop {
            let q = s.quantity()?;
            let debit = if q < s.minimum { U256::ZERO } else { quote(q)? };
            if debit == wanted {
                return Some(q);
            }
            s.advance(q, debit)?;
        }
    }
    #[test]
    fn finds_largest_partial_after_decay_and_handles_fee_and_unavailable_lots() {
        let top = U256::from(10).pow(U256::from(18));
        let c = Curve {
            quantity: top,
            output: U256::from(11),
        };
        let debit = |q: U256| Some((q * U256::from(11)).div_ceil(top));
        let q = find(&c, U256::from(9), U256::ONE, top, debit).unwrap();
        assert_eq!(q, top * U256::from(9) / U256::from(11));
        assert_eq!(debit(q), Some(U256::from(9)));
        assert_eq!(debit(q + U256::ONE), Some(U256::from(10)));
        let capped = top * U256::from(8) / U256::from(10);
        assert_eq!(
            find(&c, U256::from(9), U256::ONE, capped, debit),
            Some(capped)
        );
        assert!(find(&c, U256::from(8), capped, top, debit).is_none());
        let fee = |q: U256| {
            let raw = debit(q)?;
            Some(raw + raw.div_ceil(U256::from(7)))
        };
        assert_eq!(find(&c, U256::from(9), U256::ONE, top, fee), None);
        assert_eq!(
            find(&c, U256::from(10), U256::ONE, top, fee),
            Some(top * U256::from(8) / U256::from(11))
        );
    }
}

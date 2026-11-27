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
    pub(super) async fn sell<F: AsyncFn(U256) -> Option<U256>>(
        &self,
        wanted: U256,
        minimum: U256,
        cap: U256,
        quote: F,
    ) -> Option<U256> {
        let cap = cap.min(self.quantity);
        let mut lo = U256::ONE;
        let mut hi = wanted.min(self.output);
        let mut nominal = hi;
        let mut interpolate = true;
        while lo <= hi {
            let q = mul_div(nominal, self.quantity, self.output)?.min(cap);
            let debit = if q < minimum.max(U256::ONE) {
                U256::ZERO
            } else {
                quote(q).await?
            };
            if debit == wanted {
                return Some(q);
            }
            if debit > wanted {
                hi = nominal.checked_sub(U256::ONE)?;
            } else {
                if q == cap {
                    return None;
                }
                lo = nominal.checked_add(U256::ONE)?;
            }
            if lo > hi {
                return None;
            }
            let mid = lo + (hi - lo) / U256::from(2);
            nominal = if interpolate {
                mul_div(nominal, wanted, debit)
                    .filter(|n| *n >= lo && *n <= hi)
                    .unwrap_or(mid)
            } else {
                mid
            };
            interpolate = !interpolate;
        }
        None
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn finds_largest_partial_after_decay_and_handles_fee_and_unavailable_lots() {
        let top = U256::from(10).pow(U256::from(18));
        let c = Curve {
            quantity: top,
            output: U256::from(11),
        };
        let debit = async |q: U256| Some((q * U256::from(11)).div_ceil(top));
        let q = c.sell(U256::from(9), U256::ONE, top, debit).await.unwrap();
        assert_eq!(q, top * U256::from(9) / U256::from(11));
        assert_eq!(debit(q).await, Some(U256::from(9)));
        assert_eq!(debit(q + U256::ONE).await, Some(U256::from(10)));
        let capped = top * U256::from(8) / U256::from(10);
        assert_eq!(
            c.sell(U256::from(9), U256::ONE, capped, debit).await,
            Some(capped)
        );
        assert!(c.sell(U256::from(8), capped, top, debit).await.is_none());
        let fee = async |q: U256| {
            let raw = debit(q).await?;
            Some(raw + raw.div_ceil(U256::from(7)))
        };
        assert_eq!(c.sell(U256::from(9), U256::ONE, top, fee).await, None);
        assert_eq!(
            c.sell(U256::from(10), U256::ONE, top, fee).await,
            Some(top * U256::from(8) / U256::from(11))
        );
    }
}

//! Порядок целочисленных операций SURGE._buy и _sell. Денежный минимум при продаже
//! относится к чистой выплате, нативный метод проверяет валовой выход перед налогом.
use super::{full_math::div_up, swap_cl::Swapped};
use crate::core::types::surge::SurgeState;
use alloy_primitives::{Address, U256};

/// Для ceil(gross * mul / 100) >= net требуется gross > 100 * (net - 1) / mul.
pub fn gross_for_net(net: U256, mul: u8) -> Option<U256> {
    net.checked_sub(U256::ONE)?
        .checked_mul(U256::from(100))?
        .checked_div(U256::from(mul))?
        .checked_add(U256::ONE)
}

impl SurgeState {
    fn detail(&self, buy: bool, input: U256) -> Option<(U256, U256)> {
        if input.is_zero()
            || self.liquidity.is_zero()
            || self.stock.is_zero()
            || self.invariant.is_zero()
        {
            return None;
        }
        let den = U256::from(100);
        if buy {
            let added = input.checked_mul(U256::from(self.buy_mul))? / den;
            let qty = self
                .stock
                .checked_sub(self.invariant / self.liquidity.checked_add(added)?)?;
            (qty > U256::ONE && qty <= self.capacity).then_some((qty, added))
        } else {
            let gross = self
                .liquidity
                .checked_sub(self.invariant / self.stock.checked_add(input)?)?;
            let tax = gross.checked_mul(den.checked_sub(U256::from(self.sell_mul))?)? / den;
            let net = gross.checked_sub(tax)?;
            (!net.is_zero() && net <= self.cash).then_some((net, gross))
        }
    }
    pub fn gross(&self, input: Address, amount: U256) -> Option<U256> {
        if input != self.token {
            return None;
        }
        self.detail(false, amount).map(|(_, gross)| gross)
    }
    pub fn swap(&self, input: Address, exact_in: bool, size: U256) -> Option<Swapped> {
        let buy = input == self.wrapped;
        if (!buy && input != self.token) || size.is_zero() {
            return None;
        }
        let debit = if exact_in {
            size
        } else {
            let (out, starting, desired) = if buy {
                (self.stock, self.liquidity, size.max(U256::from(2)))
            } else {
                (
                    self.liquidity,
                    self.stock,
                    gross_for_net(size, self.sell_mul)?,
                )
            };
            // floor(k / (starting + growth)) <= out - desired.
            let target = self
                .invariant
                .checked_div(out.checked_sub(desired)?.checked_add(U256::ONE)?)?
                .checked_add(U256::ONE)?;
            let growth = target.saturating_sub(starting);
            if buy {
                div_up(
                    growth.checked_mul(U256::from(100))?,
                    U256::from(self.buy_mul),
                )?
            } else {
                growth
            }
            .max(U256::ONE)
        };
        let (received, _) = self.detail(buy, debit)?;
        if !exact_in && received < size {
            return None;
        }
        Some(Swapped {
            amount_in: debit,
            amount_out: received,
            complete: true,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn strict_inverse_respects_virtual_liquidity_real_cash_and_rounding() {
        let v: serde_json::Value = serde_json::from_reader(flate2::read::GzDecoder::new(
            &include_bytes!(
                "../../../../tests/data/first_run_dex_2026-11-26/surge_ethereum_wallets.json.gz"
            )[..],
        ))
        .unwrap();
        let raw = |n: &serde_json::Value| n.as_str().unwrap().parse::<U256>().unwrap();
        let t: Address = v["address"].as_str().unwrap().parse().unwrap();
        let w = Address::repeat_byte(1);
        let initial = SurgeState {
            token: t,
            wrapped: w,
            trader: Address::repeat_byte(2),
            liquidity: raw(&v["state"]["liquidity"]),
            stock: raw(&v["state"]["stock"]),
            invariant: raw(&v["state"]["liqConst"]),
            cash: raw(&v["cases"][0]["before"][2]),
            capacity: raw(&v["state"]["maxBag"]),
            buy_mul: 100,
            sell_mul: 100,
        };
        for r in v["cases"].as_array().unwrap() {
            let n = raw(&r["input"]);
            let bought = raw(&r["bought"]);
            assert_eq!(initial.swap(w, true, n).unwrap().amount_out, bought);
            let x = initial.swap(w, false, bought).unwrap().amount_in;
            assert!(
                x <= n
                    && initial
                        .swap(w, true, x - U256::ONE)
                        .is_none_or(|p| p.amount_out < bought)
            );
            let mut post = initial.clone();
            post.liquidity += n;
            post.stock -= bought;
            post.cash += n;
            assert_eq!(
                post.swap(t, true, bought).unwrap().amount_out,
                raw(&r["received"])
            );
            post.sell_mul = 90;
            let net = post.swap(t, true, bought).unwrap().amount_out;
            assert!(gross_for_net(net, 90).unwrap() <= n);
            let y = post.swap(t, false, net).unwrap().amount_in;
            assert!(
                y <= bought
                    && post
                        .swap(t, true, y - U256::ONE)
                        .is_none_or(|p| p.amount_out < net)
            );
            post.cash = net - U256::ONE;
            assert!(post.swap(t, true, bought).is_none());
        }
    }
}

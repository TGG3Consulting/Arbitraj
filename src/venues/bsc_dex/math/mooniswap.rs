//! Mooniswap 02dccfab и Liquidity Protocol 48d3db12. Все деления повторяют округления исходного контракта.
use super::swap_cl::Swapped;
use crate::core::types::MooniswapState;
use alloy_primitives::{Address, U256};

fn scale() -> U256 {
    U256::from(1_000_000_000_000_000_000u64)
}

// У обеих библиотек VirtualBalance.Data поле balance — uint216.
const VIRTUAL_BALANCE_BITS: usize = 216;
type Wide = alloy_primitives::Uint<768, 12>;

/// Огибающая по целому gross. При g=floor(t*y/(x+t)) доля t/(x+t) >= g/y,
/// поэтому g*(y*D-slippage*g) >= target*y*D необходимо для любой выплаты.
fn interval(y: U256, want: U256, slippage: U256) -> Option<(U256, U256)> {
    let last = y.checked_sub(U256::ONE)?;
    if want > last || slippage > scale() {
        return None;
    }
    if slippage.is_zero() {
        return Some((want, last));
    }
    let yd = Wide::from(y) * Wide::from(scale());
    let threshold = Wide::from(want) * yd;
    let value = |g: U256| Wide::from(g) * (yd - Wide::from(slippage) * Wide::from(g));
    let mut peak = if slippage <= scale() / U256::from(2) {
        last
    } else {
        super::full_math::mul_div(y, scale(), slippage * U256::from(2))?.min(last)
    };
    if peak < last && value(peak + U256::ONE) > value(peak) {
        peak += U256::ONE
    }
    if value(peak) < threshold {
        return None;
    }
    let (mut lo, mut hi) = (U256::ZERO, peak);
    while lo < hi {
        let m = lo + ((hi - lo) >> 1);
        if value(m) >= threshold {
            hi = m
        } else {
            lo = m + U256::ONE
        }
    }
    let start = lo;
    (lo, hi) = (peak, last);
    while lo < hi {
        let m = hi - ((hi - lo) >> 1);
        if value(m) >= threshold {
            lo = m
        } else {
            hi = m - U256::ONE
        }
    }
    Some((start, hi))
}

impl MooniswapState {
    fn current(&self, (value, updated): (U256, u64), real: U256) -> Option<U256> {
        let passed = self.timestamp.checked_sub(updated)?.min(self.decay);
        value
            .checked_mul(U256::from(self.decay - passed))?
            .checked_add(real.checked_mul(U256::from(passed))?)?
            .checked_div(U256::from(self.decay))
    }
    pub fn reserves(&self, input: Address) -> Option<(U256, U256)> {
        let i = self.tokens.iter().position(|&t| t == input)?;
        let x = self.balances[i];
        let y = self.balances[1 - i];
        Some((
            self.current(self.addition[i], x)?.max(x),
            self.current(self.removal[1 - i], y)?.min(y),
        ))
    }
    fn forward(&self, x: U256, y: U256, dx: U256) -> Option<(U256, U256, U256)> {
        let d = scale();
        let taxed = dx.checked_sub(dx.checked_mul(self.fee)?.checked_div(d)?)?;
        let sum = x.checked_add(taxed)?;
        let gross = taxed.checked_mul(y)?.checked_div(sum)?;
        if !self.successor {
            return Some((gross, U256::ONE, U256::ONE));
        }
        let den = d.checked_mul(sum)?;
        let num = den.checked_sub(self.slippage_fee.checked_mul(taxed)?)?;
        Some((gross.checked_mul(num)?.checked_div(den)?, num, den))
    }
    /// Рассчитывает первый полный вход. В successor floor(gross) и отдельный floor(ret)
    /// могут дать ступеньку вниз. На каждом шаге используется необходимая нижняя граница
    /// gross: коэффициент скидки к gross монотонно убывает с увеличением входа.
    pub fn swap(&self, input: Address, exact_in: bool, amount: U256) -> Option<Swapped> {
        let (x, y) = self.reserves(input)?;
        if x.is_zero() || y.is_zero() || amount.is_zero() || self.fee >= scale() {
            return None;
        }
        if exact_in {
            let out = self.forward(x, y, amount)?.0;
            return (!out.is_zero()).then_some(Swapped {
                amount_in: amount,
                amount_out: out,
                complete: true,
            });
        }
        let (mut gross, last) = interval(y, amount, self.slippage_fee)?;
        while gross <= last {
            let taxed = super::full_math::mul_div_up(gross, x, y - gross)?;
            let dx = super::full_math::mul_div(
                taxed.checked_sub(U256::ONE)?,
                scale(),
                scale() - self.fee,
            )?
            .checked_add(U256::ONE)?;
            let (out, num, den) = self.forward(x, y, dx)?;
            if out >= amount {
                return Some(Swapped {
                    amount_in: dx,
                    amount_out: out,
                    complete: true,
                });
            }
            if num.is_zero() {
                return None;
            }
            let next = super::full_math::mul_div_up(amount, den, num)?;
            if next <= gross {
                return None;
            }
            gross = next;
        }
        None
    }
    /// Обновление виртуальных направлений после полностью оплаченного входа с нулевым referral.
    pub fn after(&self, input: Address, paid: U256, out: U256) -> Option<Self> {
        let i = self.tokens.iter().position(|&a| a == input)?;
        let j = 1 - i;
        let (x, y) = self.reserves(input)?;
        if self.swap(input, true, paid)?.amount_out != out {
            return None;
        }
        let mut next = self.clone();
        let saved = |v: U256| {
            if self.successor && v.bit_len() > VIRTUAL_BALANCE_BITS {
                return None;
            }
            Some((
                v & ((U256::ONE << VIRTUAL_BALANCE_BITS) - U256::ONE),
                self.timestamp,
            ))
        };
        if x != self.balances[i] {
            next.addition[i] = saved(x.checked_add(paid)?)?
        }
        if y != self.balances[j] {
            next.removal[j] = saved(y.checked_sub(out)?)?
        }
        next.removal[i] = saved(self.current(self.removal[i], self.balances[i])?)?;
        next.addition[j] = saved(self.current(self.addition[j], self.balances[j])?)?;
        next.balances[i] = self.balances[i].checked_add(paid)?;
        next.balances[j] = self.balances[j].checked_sub(out)?;
        Some(next)
    }
}

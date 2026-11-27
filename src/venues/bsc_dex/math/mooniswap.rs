//! Mooniswap 02dccfab и Liquidity Protocol 48d3db12. Все деления повторяют округления исходного контракта.
use super::swap_cl::Swapped;
use crate::core::types::MooniswapState;
use alloy_primitives::{Address, U256};

fn scale() -> U256 {
    U256::from(1_000_000_000_000_000_000u64)
}

// У обеих библиотек VirtualBalance.Data поле balance — uint216.
const VIRTUAL_BALANCE_BITS: usize = 216;
#[path = "mooniswap_inverse.rs"]
mod inverse;

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
        inverse::first(self, x, y, amount, || {})
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

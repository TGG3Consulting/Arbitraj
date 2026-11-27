//! Штатный точный вход Deli CL. Хук забирает комиссию в wBLT и обнуляет LP-комиссию ядра.
//! Два округления при пересчёте в wBLT выполняются по начальной sqrtPriceX96.
use super::{
    full_math::mul_div_up,
    swap_cl::{self, ClState, Swapped},
};
use alloy_primitives::U256;

fn charge(
    amount: U256,
    rate: u32,
    sqrt: U256,
    fee_on_input: bool,
    zero_for_one: bool,
) -> Option<U256> {
    let basis = mul_div_up(amount, U256::from(rate), U256::from(1_000_000))?;
    if fee_on_input {
        return Some(basis);
    }
    let q = U256::ONE << 96;
    let (n, d) = if zero_for_one { (sqrt, q) } else { (q, sqrt) };
    mul_div_up(mul_div_up(basis, n, d)?, n, d)
}

fn gross(s: &ClState<'_>, dir: bool, amount: U256) -> Option<U256> {
    if amount.is_zero() {
        return Some(U256::ZERO);
    }
    let v = swap_cl::swap(s, dir, true, amount).ok()?;
    (v.complete && v.amount_in == amount).then_some(v.amount_out)
}

fn minimum(s: &ClState<'_>, dir: bool, target: U256) -> Option<U256> {
    let v = swap_cl::swap(s, dir, false, target).ok()?;
    if !v.complete
        || v.amount_out != target
        || v.amount_in.is_zero()
        || gross(s, dir, v.amount_in)? < target
    {
        return None;
    }
    let mut hi = v.amount_in;
    if gross(s, dir, hi - U256::ONE)? < target {
        return Some(hi);
    }
    let mut lo = U256::ZERO;
    while hi - lo > U256::ONE {
        let mid = lo + (hi - lo) / U256::from(2);
        if gross(s, dir, mid)? < target {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    Some(hi)
}

pub fn quote(
    s: &ClState<'_>,
    dir: bool,
    fee_on_input: bool,
    rate: u32,
    exact_in: bool,
    amount: U256,
) -> Option<Swapped> {
    let denominator = U256::from(1_000_000);
    if rate >= 1_000_000 || amount.is_zero() {
        return None;
    }
    let forward = |input: U256| {
        let fee = charge(input, rate, s.sqrt_price, fee_on_input, dir)?;
        if fee > U256::from(i128::MAX as u128) {
            return None;
        }
        if fee_on_input {
            gross(s, dir, input.checked_sub(fee)?)
        } else {
            gross(s, dir, input)?.checked_sub(fee)
        }
    };
    if exact_in {
        let out = forward(amount)?;
        return (!out.is_zero()).then_some(Swapped {
            amount_in: amount,
            amount_out: out,
            complete: true,
        });
    }
    let mut x = minimum(s, dir, amount)?;
    if fee_on_input {
        x = mul_div_up(x, denominator, denominator - U256::from(rate))?;
        return (forward(x)? >= amount).then_some(Swapped {
            amount_in: x,
            amount_out: amount,
            complete: true,
        });
    }
    // Даже если чистая выплата уменьшается на границе округления, валовый выход и сбор
    // неубывают. Каждый допустимый y >= x обязан покрыть target + charge(x), откуда
    // minimum(target+charge(x)) — новая нижняя граница. Первый прошедший x минимален.
    for _ in 0..32 {
        let fee = charge(x, rate, s.sqrt_price, false, dir)?;
        if fee > U256::from(i128::MAX as u128) {
            return None;
        }
        let target = amount.checked_add(fee)?;
        if gross(s, dir, x)? >= target {
            return Some(Swapped {
                amount_in: x,
                amount_out: amount,
                complete: true,
            });
        }
        let next = minimum(s, dir, target)?;
        if next <= x {
            return None;
        }
        x = next;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fee_rounds_up_separately_in_both_currencies() {
        let q = U256::ONE << 96;
        assert_eq!(
            charge(U256::ONE, 3000, q * 3 / 2, false, true),
            Some(U256::from(3))
        );
        assert_eq!(
            charge(U256::ONE, 3000, q * 3 / 2, false, false),
            Some(U256::ONE)
        );
        assert_eq!(
            charge(U256::from(1001), 1000, q, true, true),
            Some(U256::from(2))
        );
    }
}

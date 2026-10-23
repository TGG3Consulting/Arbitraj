//! Файл: venues/bsc_dex/math/sqrt_price_math.rs
//! Что делает: SqrtPriceMath — следующая цена после входа/выхода суммы и суммы токенов между двумя ценами,
//! с округлениями контракта. Отказ там, где контракт откатился бы, — None.
//! Кто вызывает: math/swap_step.rs.
//! От чего зависит: math/full_math.rs.
//! Настройки: нет.
//!
//! Источник: https://github.com/pancakeswap/pancake-v3-contracts/blob/986847948755cba528324d41be19480731c36c2a/projects/v3-core/contracts/libraries/SqrtPriceMath.sol#L28-L190
//! (у Uniswap V4 и Infinity CL — те же функции и округления: research/flow/bsc_v4.md, раздел 5).
//! FixedPoint96: RESOLUTION = 96, Q96 = 2^96.

use alloy_primitives::U256;

use super::full_math::{div_up, mul_div, mul_div_up};

/// FixedPoint96.RESOLUTION.
const RESOLUTION: usize = 96;

/// 2^160 − 1 (type(uint160).max).
fn max_u160() -> U256 {
    (U256::from(1u8) << 160) - U256::from(1u8)
}

/// Цена после добавления/снятия amount токена0 (округление вверх).
fn next_from_amount0_up(sqrt_p: U256, liquidity: u128, amount: U256, add: bool) -> Option<U256> {
    if amount.is_zero() {
        return Some(sqrt_p);
    }
    let n1 = U256::from(liquidity) << RESOLUTION;
    if add {
        let (product, over) = amount.overflowing_mul(sqrt_p);
        if !over {
            let (den, over2) = n1.overflowing_add(product);
            if !over2 {
                return mul_div_up(n1, sqrt_p, den);
            }
        }
        div_up(n1, (n1 / sqrt_p).checked_add(amount)?)
    } else {
        let (product, over) = amount.overflowing_mul(sqrt_p);
        if over || n1 <= product {
            return None;
        }
        let r = mul_div_up(n1, sqrt_p, n1 - product)?;
        (r <= max_u160()).then_some(r)
    }
}

/// Цена после добавления/снятия amount токена1 (округление вниз).
fn next_from_amount1_down(sqrt_p: U256, liquidity: u128, amount: U256, add: bool) -> Option<U256> {
    let l = U256::from(liquidity);
    let q96 = U256::from(1u8) << RESOLUTION;
    if add {
        let q = if amount <= max_u160() {
            (amount << RESOLUTION) / l
        } else {
            mul_div(amount, q96, l)?
        };
        let r = sqrt_p.checked_add(q)?;
        (r <= max_u160()).then_some(r)
    } else {
        let q = if amount <= max_u160() {
            div_up(amount << RESOLUTION, l)?
        } else {
            mul_div_up(amount, q96, l)?
        };
        (sqrt_p > q).then(|| sqrt_p - q)
    }
}

/// getNextSqrtPriceFromInput. None — цена или ликвидность 0, выход за пределы.
pub fn next_from_input(
    sqrt_p: U256,
    liquidity: u128,
    amount_in: U256,
    zero_for_one: bool,
) -> Option<U256> {
    if sqrt_p.is_zero() || liquidity == 0 {
        return None;
    }
    if zero_for_one {
        next_from_amount0_up(sqrt_p, liquidity, amount_in, true)
    } else {
        next_from_amount1_down(sqrt_p, liquidity, amount_in, true)
    }
}

/// getNextSqrtPriceFromOutput. None — цена или ликвидность 0, выход за пределы.
pub fn next_from_output(
    sqrt_p: U256,
    liquidity: u128,
    amount_out: U256,
    zero_for_one: bool,
) -> Option<U256> {
    if sqrt_p.is_zero() || liquidity == 0 {
        return None;
    }
    if zero_for_one {
        next_from_amount1_down(sqrt_p, liquidity, amount_out, false)
    } else {
        next_from_amount0_up(sqrt_p, liquidity, amount_out, false)
    }
}

/// getAmount0Delta (беззнаковый). None — нижняя цена 0 или переполнение.
pub fn amount0_delta(a: U256, b: U256, liquidity: u128, round_up: bool) -> Option<U256> {
    let (a, b) = if a > b { (b, a) } else { (a, b) };
    if a.is_zero() {
        return None;
    }
    let n1 = U256::from(liquidity) << RESOLUTION;
    let n2 = b - a;
    if round_up {
        div_up(mul_div_up(n1, n2, b)?, a)
    } else {
        Some(mul_div(n1, n2, b)? / a)
    }
}

/// getAmount1Delta (беззнаковый). None — переполнение.
pub fn amount1_delta(a: U256, b: U256, liquidity: u128, round_up: bool) -> Option<U256> {
    let (a, b) = if a > b { (b, a) } else { (a, b) };
    let q96 = U256::from(1u8) << RESOLUTION;
    if round_up {
        mul_div_up(U256::from(liquidity), b - a, q96)
    } else {
        mul_div(U256::from(liquidity), b - a, q96)
    }
}

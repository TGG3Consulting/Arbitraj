//! Файл: venues/bsc_dex/math/crypto_ops.rs
//! Что делает: арифметика Vyper 0.3.10 для математики crypto-пулов Curve (решение 442) — то, как контракт считает
//! целые: обычные `+ - * / **` и `convert` проверяются (переполнение, деление на ноль, отрицательное в uint256 —
//! откат → None), `unsafe_*` и `pow_mod256` — по модулю 2^256 как опкоды EVM (ADD/SUB/MUL, DIV/SDIV: деление на ноль
//! даёт 0), деление со знаком — к нулю (SDIV). Плюс общие части обеих математик: `_snekmate_log_2`, `_cbrt`, `isqrt`.
//! Кто вызывает: math/crypto_y2.rs, math/crypto_y3.rs, math/crypto_newton3.rs, math/crypto_swap.rs.
//! От чего зависит: alloy-primitives (U256, I256), math/ten.rs.
//! Настройки: нет (числа — константы исходника контракта).
//!
//! Источник: curvefi/twocrypto-ng 89c8dfb (= тег prod-v2.1.0), contracts/main/CurveCryptoMathOptimized2.vy —
//! `_snekmate_log_2` L35-L84, `_cbrt` L87-L140; curvefi/tricrypto-ng ecaa816, contracts/main/CurveCryptoMathOptimized3.vy
//! — те же функции L737-L842 (тексты совпадают построчно). `isqrt` — встроенная функция Vyper 0.3.10 (целый корень вниз).

use alloy_primitives::{I256, U256};

use super::ten;

/// Целое со знаком из неотрицательного U256 (константы исходника). Ошибок нет (значения < 2^255).
pub fn i(v: U256) -> I256 {
    I256::from_raw(v)
}

/// Целое со знаком из u64 (малые константы исходника). Ошибок нет.
pub fn iu(v: u64) -> I256 {
    I256::from_raw(U256::from(v))
}

/// 10^n со знаком (константы исходника, n ≤ 76). Ошибок нет.
pub fn ie(n: u64) -> I256 {
    I256::from_raw(ten::pow(n))
}

/// Проверяемое `a * b` int256. None — переполнение (откат).
pub fn mul(a: I256, b: I256) -> Option<I256> {
    a.checked_mul(b)
}

/// Проверяемое `a + b` int256. None — переполнение.
pub fn add(a: I256, b: I256) -> Option<I256> {
    a.checked_add(b)
}

/// Проверяемое `a - b` int256. None — переполнение.
pub fn sub(a: I256, b: I256) -> Option<I256> {
    a.checked_sub(b)
}

/// Проверяемое `a / b` int256 (к нулю). None — деление на ноль или MIN / −1.
pub fn div(a: I256, b: I256) -> Option<I256> {
    a.checked_div(b)
}

/// Проверяемое `a ** 2` int256. None — переполнение.
pub fn sq(a: I256) -> Option<I256> {
    a.checked_mul(a)
}

/// `abs(a)` Vyper: None — a = MIN (откат).
pub fn abs(a: I256) -> Option<I256> {
    a.checked_abs()
}

/// `unsafe_mul` int256 — по модулю 2^256. Ошибок нет.
pub fn umul(a: I256, b: I256) -> I256 {
    a.wrapping_mul(b)
}

/// `unsafe_add` int256. Ошибок нет.
pub fn uadd(a: I256, b: I256) -> I256 {
    a.wrapping_add(b)
}

/// `unsafe_sub` int256. Ошибок нет.
pub fn usub(a: I256, b: I256) -> I256 {
    a.wrapping_sub(b)
}

/// `unsafe_div` int256 — опкод SDIV: деление на ноль даёт 0, MIN / −1 даёт MIN. Ошибок нет.
pub fn udiv(a: I256, b: I256) -> I256 {
    if b.is_zero() {
        I256::ZERO
    } else {
        a.wrapping_div(b)
    }
}

/// `convert(a, uint256)`: None — a < 0 (откат).
pub fn to_u(a: I256) -> Option<U256> {
    (!a.is_negative()).then(|| a.into_raw())
}

/// `convert(a, int256)`: None — a ≥ 2^255 (откат).
pub fn to_i(a: U256) -> Option<I256> {
    I256::try_from(a).ok()
}

/// `unsafe_div` uint256 — опкод DIV: деление на ноль даёт 0. Ошибок нет.
pub fn udiv_u(a: U256, b: U256) -> U256 {
    a.checked_div(b).unwrap_or(U256::ZERO)
}

/// Проверяемое `a * b` uint256. None — переполнение.
pub fn mu(a: U256, b: U256) -> Option<U256> {
    a.checked_mul(b)
}

/// Проверяемое `a / b` uint256. None — деление на ноль.
pub fn du(a: U256, b: U256) -> Option<U256> {
    a.checked_div(b)
}

/// Проверяемое `a + b` uint256. None — переполнение.
pub fn au(a: U256, b: U256) -> Option<U256> {
    a.checked_add(b)
}

/// Проверяемое `a - b` uint256. None — меньше нуля.
pub fn su(a: U256, b: U256) -> Option<U256> {
    a.checked_sub(b)
}

/// `isqrt(x)` Vyper — целый квадратный корень вниз. Ошибок нет.
pub fn isqrt(x: U256) -> U256 {
    x.root(2)
}

/// `_snekmate_log_2(x, False)` — log2 вниз (0 при x = 0). Ошибок нет.
pub fn log2(x: U256) -> u64 {
    if x.is_zero() {
        0
    } else {
        // Номер старшего единичного бита — то же, что лестница сдвигов исходника (L49-L77).
        (x.bit_len() - 1) as u64
    }
}

/// Порог 115792089237316195423570985008687907853269 (≈ 2^256 / 10^36) из `_cbrt`. Ошибок нет.
fn cbrt_limit() -> U256 {
    // 115792089237316195423570985008687907853269 = 115792089237316 · 10^27 + 195423570985008687907853269.
    U256::from(115_792_089_237_316u64)
        .wrapping_mul(ten::pow(27))
        .wrapping_add(U256::from(195_423_570_985_008_687_907_853_269u128))
}

/// `_cbrt(x)` — кубический корень в фикс-точке 18 знаков: начальное приближение по log2 и 7 шагов Ньютона без
/// проверок (unsafe_*), как в исходнике L87-L140. Ошибок нет (нет проверяемых операций).
pub fn cbrt(x: U256) -> U256 {
    let lim = cbrt_limit();
    let big = lim.wrapping_mul(ten::E18);
    let xx = if x >= big {
        x
    } else if x >= lim {
        x.wrapping_mul(ten::E18)
    } else {
        x.wrapping_mul(ten::E36)
    };
    let log2x = log2(xx);
    let rem = log2x % 3;
    let pow = |b: u64, e: u64| U256::from(b).wrapping_pow(U256::from(e));
    let guess = pow(2, log2x / 3).wrapping_mul(pow(1260, rem));
    let mut a = udiv_u(guess, pow(1000, rem));
    let three = U256::from(3u8);
    for _ in 0..7 {
        let t = U256::from(2u8)
            .wrapping_mul(a)
            .wrapping_add(udiv_u(xx, a.wrapping_mul(a)));
        a = udiv_u(t, three);
    }
    if x >= big {
        a = a.wrapping_mul(ten::pow(12));
    } else if x >= lim {
        a = a.wrapping_mul(ten::pow(6));
    }
    a
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Порог `_cbrt` — та же строка, что в исходнике.
    #[test]
    fn cbrt_limit_matches_source() {
        let text = "115792089237316195423570985008687907853269";
        let v: U256 = text.parse().unwrap();
        assert_eq!(cbrt_limit(), v);
    }

    /// Кубический корень в 18 знаках: cbrt(8·10^18) = 2·10^18 (фикс-точка: x·10^36 под корнем).
    #[test]
    fn cbrt_exact_cube() {
        let eight = U256::from(8u8) * ten::E18;
        let two = U256::from(2u8) * ten::E18;
        assert_eq!(cbrt(eight), two);
        assert_eq!(cbrt(ten::E18), ten::E18);
    }

    /// SDIV: деление на ноль — 0, к нулю у отрицательных.
    #[test]
    fn signed_division_like_evm() {
        assert_eq!(udiv(iu(7), I256::ZERO), I256::ZERO);
        let (m7, m3) = (I256::ZERO - iu(7), I256::ZERO - iu(3));
        assert_eq!(udiv(m7, iu(2)), m3);
        assert_eq!(div(m7, iu(2)), Some(m3));
        assert_eq!(div(iu(1), I256::ZERO), None);
    }
}

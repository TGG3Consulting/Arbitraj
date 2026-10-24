//! Файл: venues/bsc_dex/math/full_math.rs
//! Что делает: умножение с делением без переполнения (512-битное промежуточное) с округлением вниз и вверх,
//! деление с округлением вверх, сдвиги 128.128 — как библиотеки контрактов. Переполнение результата сверх 256 бит
//! или деление на ноль — None (там контракт откатился бы).
//! Кто вызывает: math/sqrt_price_math.rs, swap_step.rs, bin_swap.rs.
//! От чего зависит: alloy-primitives (U256, U512).
//! Настройки: нет.
//!
//! Источники: FullMath.mulDiv / mulDivRoundingUp —
//! https://github.com/pancakeswap/pancake-v3-contracts/blob/986847948755cba528324d41be19480731c36c2a/projects/v3-core/contracts/libraries/FullMath.sol ;
//! UnsafeMath.divRoundingUp — …/libraries/UnsafeMath.sol;
//! Uint256x256Math.mulShiftRoundDown/Up, shiftDivRoundDown/Up —
//! https://github.com/pancakeswap/infinity-core/blob/3c5ef1be1f71442d8bc60cc6fd11f9c677b714df/src/pool-bin/libraries/math/Uint256x256Math.sol#L57-L127 .

use alloy_primitives::{U256, U512};

/// floor(a·b / d). None — d = 0 или результат больше 2^256 − 1 (FullMath: require).
pub fn mul_div(a: U256, b: U256, d: U256) -> Option<U256> {
    if d.is_zero() {
        return None;
    }
    let r = U512::from(a) * U512::from(b) / U512::from(d);
    narrow(r)
}

/// ceil(a·b / d). None — d = 0 или результат больше 2^256 − 1.
pub fn mul_div_up(a: U256, b: U256, d: U256) -> Option<U256> {
    if d.is_zero() {
        return None;
    }
    let p = U512::from(a) * U512::from(b);
    let d = U512::from(d);
    let mut r = p / d;
    if !(p % d).is_zero() {
        r += U512::from(1u8);
    }
    narrow(r)
}

/// ceil(a / d) без проверок переполнения (UnsafeMath.divRoundingUp). None — d = 0.
pub fn div_up(a: U256, d: U256) -> Option<U256> {
    if d.is_zero() {
        return None;
    }
    let r = a / d;
    Some(if (a % d).is_zero() {
        r
    } else {
        r + U256::from(1u8)
    })
}

/// floor(x·y / 2^offset). None — результат больше 2^256 − 1 (MulShiftOverflow).
pub fn mul_shift(x: U256, y: U256, offset: usize, up: bool) -> Option<U256> {
    let p = U512::from(x) * U512::from(y);
    let mut r = p >> offset;
    if up && !(p & ((U512::from(1u8) << offset) - U512::from(1u8))).is_zero() {
        r += U512::from(1u8);
    }
    narrow(r)
}

/// floor((x · 2^offset) / d) (или ceil при up). None — d = 0 или результат больше 2^256 − 1.
pub fn shift_div(x: U256, offset: usize, d: U256, up: bool) -> Option<U256> {
    if d.is_zero() {
        return None;
    }
    let p = U512::from(x) << offset;
    let dd = U512::from(d);
    let mut r = p / dd;
    if up && !(p % dd).is_zero() {
        r += U512::from(1u8);
    }
    narrow(r)
}

/// U512 → U256, если помещается.
fn narrow(r: U512) -> Option<U256> {
    if r > U512::from(U256::MAX) {
        return None;
    }
    Some(U256::from_limbs_slice(&r.as_limbs()[..4]))
}

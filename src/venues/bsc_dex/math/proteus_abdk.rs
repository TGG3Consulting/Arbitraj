//! Файл: venues/bsc_dex/math/proteus_abdk.rs
//! Что делает: решение 486 — порт функций ABDKMath64x64, которыми считает реализация Proteus (Shell Protocol): sub,
//! mul, inv (64.64 × 64.64), muli (64.64 × int256 → int256), mulu, divi (int256 / int256 → 64.64), divuu. Числа 64.64
//! (int128 ABDK) и int256 — в I256; «откат» исходника (require, деление на ноль) — None. Поведение «We rely on
//! overflow behavior» исходника (−MIN как uint256 = 2^255) — через unsigned_abs (тот же результат).
//! Кто вызывает: math/proteus_curve.rs, math/proteus_swap.rs.
//! От чего зависит: alloy-primitives (I256, U256).
//! Настройки: нет (пределы 64.64 — константы библиотеки).
//!
//! Источник (решение 23): ABDKMath64x64.sol — github.com/cowri/Proteus@027b5ec contracts/ABDKMath64x64.sol (git-блоб
//! bfef6d78 = копия в исходниках Sourcify 42161 реализаций v1/v2-группы 0x537e2d22…, 0x243e8336… и др.):
//! MIN_64x64 / MAX_64x64 L20, L25; sub L132-L138; mul L147-L153; muli L163-L191; mulu L201-L218; divi L244-L266;
//! inv L318-L325; divuu L677-L717 — divuu этой копии кончается `assert (xh == hi >> 128); result += xl / y;`.
//! Реализация SHELL+ETH 0xf6e9c132 (Ocean v3) несёт более новую копию ABDK, у которой divuu вместо assert делает
//! `result += xh == hi >> 128 ? xl / y : 1` (прочие функции совпадают, сверка Sourcify 20.11.2026). Порт — форма с
//! assert: при xh != hi>>128 возвращает None. Эта ветка у обеих копий недостижима для входов, прошедших предыдущий
//! `require (result <= 2^128 − 1)`, при реальных балансах пула (приведённый учёт, 18 знаков) — расчёт совпадает с
//! обеими развёрнутыми копиями на всех достижимых суммах.

use alloy_primitives::{I256, U256};

/// MIN_64x64 = −2^127 (L20).
fn min64() -> I256 {
    I256::from_raw(U256::MAX << 127)
}

/// MAX_64x64 = 2^127 − 1 (L25).
fn max64() -> I256 {
    I256::from_raw((U256::from(1u8) << 127) - U256::from(1u8))
}

/// Лежит ли значение в пределах int128 (64.64). Возвращает: само значение или None (require исходника).
fn in64(v: I256) -> Option<I256> {
    (v >= min64() && v <= max64()).then_some(v)
}

/// sub(x, y) = x − y в пределах 64.64 (L132-L138). None — вне пределов.
pub fn sub(x: I256, y: I256) -> Option<I256> {
    in64(x.checked_sub(y)?)
}

/// Деление на 2^64 с округлением вниз (к −∞) — сдвиг `>> 64` знакового int256 Solidity. Ошибок нет.
fn shr64_floor(p: I256) -> I256 {
    let d = I256::from_raw(U256::from(1u8) << 64);
    let q = p.checked_div(d).unwrap_or(I256::ZERO);
    if p.is_negative() && q.checked_mul(d) != Some(p) {
        q - I256::ONE
    } else {
        q
    }
}

/// mul(x, y) = (x · y) >> 64 в пределах 64.64 (L147-L153). None — вне пределов.
pub fn mul(x: I256, y: I256) -> Option<I256> {
    in64(shr64_floor(x.checked_mul(y)?))
}

/// inv(x) = 2^128 / x (к нулю) в пределах 64.64 (L318-L325). None — x = 0 или вне пределов.
pub fn inv(x: I256) -> Option<I256> {
    if x.is_zero() {
        return None;
    }
    let n = I256::from_raw(U256::from(1u8) << 128);
    in64(n.checked_div(x)?)
}

/// mulu(x, y) — 64.64 × uint256 → uint256 вниз (L201-L218). None — x < 0 или переполнение (require исходника).
pub fn mulu(x: I256, y: U256) -> Option<U256> {
    if y.is_zero() {
        return Some(U256::ZERO);
    }
    if x.is_negative() {
        return None;
    }
    let xu = x.into_raw();
    let mask = (U256::from(1u8) << 128) - U256::from(1u8);
    let lo = (xu * (y & mask)) >> 64;
    let mut hi = xu * (y >> 128);
    if hi > (U256::from(1u8) << 192) - U256::from(1u8) {
        return None;
    }
    hi <<= 64;
    if hi > U256::MAX - lo {
        return None;
    }
    Some(hi + lo)
}

/// muli(x, y) — 64.64 × int256 → int256 к нулю (L163-L191). None — переполнение (require исходника).
pub fn muli(x: I256, y: I256) -> Option<I256> {
    if x == min64() {
        // L165-L168: y в [−0xFF…F (48 F), 0x1 и 48 нулей] → −y << 63.
        let lo = I256::from_raw((U256::from(1u8) << 192) - U256::from(1u8)).checked_neg()?;
        let hi = I256::from_raw(U256::from(1u8) << 192);
        if y < lo || y > hi {
            return None;
        }
        return y.checked_neg()?.checked_mul(I256::from_raw(U256::from(1u8) << 63));
    }
    let negative = x.is_negative() != y.is_negative();
    let abs = mulu(I256::from_raw(x.unsigned_abs()), y.unsigned_abs())?;
    let top = U256::from(1u8) << 255;
    if negative && !abs.is_zero() {
        if abs > top {
            return None;
        }
        // −int256(abs): при abs = 2^255 — MIN (опора исходника на перенос).
        Some(I256::from_raw(abs.wrapping_neg()))
    } else {
        if abs > top - U256::from(1u8) {
            return None;
        }
        Some(I256::from_raw(abs))
    }
}

/// divuu(x, y) — uint256 / uint256 → 64.64 без знака, к нулю (L677-L717). None — y = 0 или результат > 2^128 − 1.
pub fn divuu(x: U256, y: U256) -> Option<U256> {
    if y.is_zero() {
        return None;
    }
    let one = U256::from(1u8);
    let m128 = (one << 128) - one;
    let result = if x <= (one << 192) - one {
        (x << 64) / y
    } else {
        let mut msb: usize = 192;
        let mut xc = x >> 192;
        let steps: [(u64, usize); 5] =
            [(1 << 32, 32), (1 << 16, 16), (1 << 8, 8), (1 << 4, 4), (4, 2)];
        for (lim, sh) in steps {
            if xc >= U256::from(lim) {
                xc >>= sh;
                msb += sh;
            }
        }
        if xc >= U256::from(2u8) {
            msb += 1;
        }
        let mut r = (x << (255 - msb)) / (((y - one) >> (msb - 191)) + one);
        if r > m128 {
            return None;
        }
        let hi = r.wrapping_mul(y >> 128);
        let lo = r.wrapping_mul(y & m128);
        let mut xh: U256 = x >> 192;
        let mut xl: U256 = x << 64;
        if xl < lo {
            xh = xh.wrapping_sub(one);
        }
        xl = xl.wrapping_sub(lo);
        let lo2 = hi << 128;
        if xl < lo2 {
            xh = xh.wrapping_sub(one);
        }
        xl = xl.wrapping_sub(lo2);
        // assert(xh == hi >> 128) исходника (L710).
        if xh != hi >> 128 {
            return None;
        }
        r += xl / y;
        r
    };
    (result <= m128).then_some(result)
}

/// divi(x, y) — int256 / int256 → 64.64 к нулю (L244-L266). None — y = 0 или вне пределов 64.64.
pub fn divi(x: I256, y: I256) -> Option<I256> {
    if y.is_zero() {
        return None;
    }
    let negative = x.is_negative() != y.is_negative();
    let abs = divuu(x.unsigned_abs(), y.unsigned_abs())?;
    let top = U256::from(1u8) << 127;
    if negative && !abs.is_zero() {
        (abs <= top).then(|| I256::from_raw(abs.wrapping_neg()))
    } else {
        (abs < top).then(|| I256::from_raw(abs))
    }
}

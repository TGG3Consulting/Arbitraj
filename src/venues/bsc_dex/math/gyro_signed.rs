//! Файл: venues/bsc_dex/math/gyro_signed.rs
//! Что делает: решение 452 — порт SignedFixedPoint Gyro (int256, 18 знаков ONE и 38 знаков ONE_XP): сложение,
//! вычитание, умножение и деление с округлением к нулю («Down») и от нуля («Up») по модулю, расширенная точность (Xp) и
//! перевод из неё в обычную. Solidity 0.8 — проверяемая арифметика и у функций «U» (без своей проверки): переполнение,
//! деление на ноль, MIN / −1 — None. Деление int256 — к нулю, остаток — со знаком делимого (как I256 checked_div /
//! checked_rem alloy-primitives).
//! Кто вызывает: math/gyro_eclp_inv.rs, math/gyro_eclp.rs.
//! От чего зависит: alloy-primitives (I256).
//! Настройки: нет.
//!
//! Источник (решение 23): balancer/balancer-deployments@b55dc2b8 build-info v3/tasks/20260126-v3-gyro-eclp-v2 →
//! contracts/lib/SignedFixedPoint.sol (gyrostable/concentrated-lps): ONE L22, ONE_XP L25, mulDownMag(U) L49-L62,
//! mulUpMag(U) L65-L92, divDownMag(U) L95-L115, divUpMag(U) L118-L159, mulXp(U) L166-L181, divXp(U) L187-L210,
//! mulDownXpToNp(U) L216-L238, mulUpXpToNp(U) L244-L266 (файл побайтно равен в 20250124-v3-gyro-eclp).

use alloy_primitives::{I256, U256};

/// Малое целое как I256. Ошибок нет.
pub fn int(n: i64) -> I256 {
    I256::unchecked_from(n)
}

/// 10^n как I256 (n ≤ 38). Ошибок нет.
pub fn ten(n: u64) -> I256 {
    I256::from_raw(super::ten::pow(n))
}

/// ONE = 1e18.
pub fn one() -> I256 {
    ten(18)
}

/// ONE_XP = 1e38.
pub fn one_xp() -> I256 {
    ten(38)
}

/// a + b (с откатом).
pub fn add(a: I256, b: I256) -> Option<I256> {
    a.checked_add(b)
}

/// a − b (с откатом).
pub fn sub(a: I256, b: I256) -> Option<I256> {
    a.checked_sub(b)
}

/// a · b (с откатом).
pub fn mul(a: I256, b: I256) -> Option<I256> {
    a.checked_mul(b)
}

/// a / b — к нулю (деление на ноль и MIN / −1 — откат).
pub fn div(a: I256, b: I256) -> Option<I256> {
    a.checked_div(b)
}

/// −a (с откатом на MIN).
pub fn neg(a: I256) -> Option<I256> {
    a.checked_neg()
}

/// mulDownMag / mulDownMagU: (a·b) / ONE — к нулю.
pub fn mul_down_mag(a: I256, b: I256) -> Option<I256> {
    div(mul(a, b)?, one())
}

/// mulUpMag / mulUpMagU: от нуля — p > 0: (p − 1) / ONE + 1; p < 0: (p + 1) / ONE − 1; 0 — 0.
pub fn mul_up_mag(a: I256, b: I256) -> Option<I256> {
    up(mul(a, b)?, one())
}

/// Деление p / d (d > 0) от нуля: как mulUpMag (L72-L75). Ошибок нет кроме переполнения.
fn up(p: I256, d: I256) -> Option<I256> {
    if p.is_positive() {
        add(div(sub(p, I256::ONE)?, d)?, I256::ONE)
    } else if p.is_negative() {
        sub(div(add(p, I256::ONE)?, d)?, I256::ONE)
    } else {
        Some(I256::ZERO)
    }
}

/// divDownMag / divDownMagU: (a·ONE) / b — к нулю; b = 0 — откат; a = 0 — 0.
pub fn div_down_mag(a: I256, b: I256) -> Option<I256> {
    if b.is_zero() {
        return None;
    }
    if a.is_zero() {
        return Some(I256::ZERO);
    }
    div(mul(a, one())?, b)
}

/// divUpMag / divUpMagU (L118-L159): b < 0 — знаки обоих меняются; затем от нуля: a·ONE > 0 — (a·ONE − 1) / b + 1, иначе
/// (a·ONE + 1) / b − 1; b = 0 — откат; a = 0 — 0.
pub fn div_up_mag(a: I256, b: I256) -> Option<I256> {
    if b.is_zero() {
        return None;
    }
    if a.is_zero() {
        return Some(I256::ZERO);
    }
    let (a, b) = if b.is_negative() {
        (neg(a)?, neg(b)?)
    } else {
        (a, b)
    };
    up(mul(a, one())?, b)
}

/// mulXp / mulXpU: (a·b) / ONE_XP.
pub fn mul_xp(a: I256, b: I256) -> Option<I256> {
    div(mul(a, b)?, one_xp())
}

/// divXp / divXpU: (a·ONE_XP) / b; b = 0 — откат.
pub fn div_xp(a: I256, b: I256) -> Option<I256> {
    if b.is_zero() {
        return None;
    }
    div(mul(a, one_xp())?, b)
}

/// Части множителя b = b1·1e19 + b2 и произведения prod1 = a·b1, prod2 = a·b2 (L217-L222, L245-L250).
fn split(a: I256, b: I256) -> Option<(I256, I256, I256)> {
    let e19 = ten(19);
    let b1 = div(b, e19)?;
    let b2 = b.checked_rem(e19)?;
    Some((mul(a, b1)?, mul(a, b2)?, e19))
}

/// mulDownXpToNp / mulDownXpToNpU (L216-L238): оба произведения ≥ 0 — (p1 + p2 / 1e19) / 1e19, иначе
/// (p1 + p2 / 1e19 + 1) / 1e19 − 1.
pub fn mul_down_xp_to_np(a: I256, b: I256) -> Option<I256> {
    let (p1, p2, e19) = split(a, b)?;
    let s = add(p1, div(p2, e19)?)?;
    if !p1.is_negative() && !p2.is_negative() {
        div(s, e19)
    } else {
        sub(div(add(s, I256::ONE)?, e19)?, I256::ONE)
    }
}

/// mulUpXpToNp / mulUpXpToNpU (L244-L266): оба произведения ≤ 0 — (p1 + p2 / 1e19) / 1e19, иначе
/// (p1 + p2 / 1e19 − 1) / 1e19 + 1.
pub fn mul_up_xp_to_np(a: I256, b: I256) -> Option<I256> {
    let (p1, p2, e19) = split(a, b)?;
    let s = add(p1, div(p2, e19)?)?;
    if !p1.is_positive() && !p2.is_positive() {
        div(s, e19)
    } else {
        add(div(sub(s, I256::ONE)?, e19)?, I256::ONE)
    }
}

/// int256 → uint256 (SafeCast.toUint256: отрицательное — откат).
pub fn to_u(a: I256) -> Option<U256> {
    (!a.is_negative()).then_some(a.into_raw())
}

/// uint256 → int256 (SafeCast.toInt256: больше int256 max — откат).
pub fn to_i(a: U256) -> Option<I256> {
    I256::try_from(a).ok()
}

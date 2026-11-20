//! Файл: venues/bsc_dex/math/proteus_curve.rs
//! Что делает: решение 486 — кривая Proteus (Shell Protocol): полезность точки в срезе (_getUtility, две формулы
//! развёрнутых реализаций — V1 и V2), вторая координата точки по одной координате и полезности
//! (_getPointGivenXandUtility / _getPointGivenYandUtility), поиск среза (_findSlice), границы среза
//! (_getSliceBoundaries), «точка не в срезе» (_pointIsNotInSlice), соседний срез (_next) и поиск конечной точки
//! (_findFinalPoint). Арифметика int256 Solidity 0.8 — проверяемая (переполнение, деление на ноль, MIN / −1 — None);
//! явное приведение int256 → uint256 перед корнем — без проверки (как в исходнике: биты как есть). Откаты исходника
//! (CurveError, BoundaryError, BalanceError) — None.
//! Кто вызывает: math/proteus_swap.rs.
//! От чего зависит: math/proteus_abdk.rs, alloy-primitives, core/types (ProteusUtility).
//! Настройки: нет (константы — исходника реализации).
//!
//! Источник (решение 23): Shell-Protocol/Shell-Protocol@bfbba01 src/proteus/Proteus.sol (= исходник Sourcify 42161
//! реализаций 0x537e2d22… DAI+USDC и 0xd5a7edb9… USDT+USDC, git-блоб 6ad85288): MIN_BALANCE L19, MAX_M L22, MIN_M L25,
//! ABDK_ONE / ABDK_TWO L16-L17, _findFinalPoint L507-L525, _getUtility L547-L568 (формула V1),
//! _getPointGivenXandUtility L582-L597, _getPointGivenYandUtility L611-L624, _findSlice L642-L666,
//! _pointIsNotInSlice L675-L687, _getSliceBoundaries L699-L718, _next L729-L745. Формула V2 — исходник Sourcify
//! реализаций 0x243e8336… (ARB+ETH) и других (git-блоб 3eb2c0c1) Proteus.sol L551-L570 и v3 0xf6e9c132… (SHELL+ETH)
//! L349-L367; прочий код этих реализаций с bfbba01 совпадает (отличаются только BASE_FEE и _getUtility).

use alloy_primitives::{I256, U256};

use crate::core::types::ProteusUtility;

use super::proteus_abdk::{divi, inv, mul, muli, sub};

/// Малое целое как I256. Ошибок нет.
pub fn int(n: i64) -> I256 {
    I256::unchecked_from(n)
}

/// MIN_BALANCE = 10^12 (L19).
pub fn min_balance() -> I256 {
    int(1_000_000_000_000)
}

/// MAX_M = 0x5f5e1000000000000000000 = 10^8 · 2^64 (L22).
pub fn max_m() -> I256 {
    I256::from_raw(U256::from(100_000_000u64) << 64)
}

/// MIN_M = 0x00000000000002af31dc461 (L25).
pub fn min_m() -> I256 {
    int(0x2af31dc461)
}

/// ABDK_ONE = 1 << 64 (L16).
fn abdk_one() -> I256 {
    I256::from_raw(U256::from(1u8) << 64)
}

/// ABDK_TWO = 2 << 64 (L17).
fn abdk_two() -> I256 {
    I256::from_raw(U256::from(2u8) << 64)
}

/// int256(Math.sqrt(uint256(v))): приведение без проверки (биты как есть), корень вниз. Ошибок нет.
fn sqrt_cast(v: I256) -> I256 {
    I256::from_raw(v.into_raw().root(2))
}

/// Срез: a, b, k (64.64).
#[derive(Debug, Clone, Copy)]
pub struct Slice {
    pub a: I256,
    pub b: I256,
    pub k: I256,
}

/// _getUtility — полезность точки (x, y) в срезе. Формула V1 — L547-L568 bfbba01, V2 — L551-L570 (см. шапку).
/// Возвращает: полезность (≥ 0) или None (откат: переполнение, CurveError).
pub fn utility(f: ProteusUtility, x: I256, y: I256, s: Slice) -> Option<I256> {
    let u = match f {
        ProteusUtility::V1 => {
            let ay = muli(s.a, y)?;
            let bx = muli(s.b, x)?;
            let two = abdk_two();
            let den_inv = inv(mul(s.k, sub(two, mul(two, mul(s.a, s.b)?)?)?)?)?;
            let d = bx
                .checked_mul(bx)?
                .checked_add(int(4).checked_mul(x)?.checked_mul(y)?)?
                .checked_add(ay.checked_mul(ay)?)?
                .checked_sub(int(2).checked_mul(ay)?.checked_mul(bx)?)?;
            let disc = sqrt_cast(d);
            let r0 = muli(den_inv, disc.checked_add(ay)?.checked_add(bx)?)?;
            let r1 = muli(den_inv, disc.checked_neg()?.checked_add(ay)?.checked_add(bx)?)?;
            r0.max(r1)
        }
        ProteusUtility::V2 => {
            let e18 = int(1_000_000_000_000_000_000);
            let a_q = muli(mul(s.k, sub(mul(s.a, s.b)?, abdk_one())?)?, e18)?;
            let b_q = muli(s.a, y)?.checked_add(muli(s.b, x)?)?;
            let c_q = x.checked_mul(y)?.checked_div(muli(s.k, e18)?)?;
            let d = b_q
                .checked_mul(b_q)?
                .checked_sub(a_q.checked_mul(c_q)?.checked_mul(int(4))?)?;
            let disc = sqrt_cast(d);
            let den = int(2).checked_mul(a_q)?;
            let r0 = b_q.checked_neg()?.checked_add(disc)?.checked_mul(e18)?.checked_div(den)?;
            let r1 = b_q.checked_neg()?.checked_sub(disc)?.checked_mul(e18)?.checked_div(den)?;
            if s.a.is_negative() && s.b.is_negative() {
                r0.min(r1)
            } else {
                r0.max(r1)
            }
        }
    };
    (!u.is_negative()).then_some(u)
}

/// _getPointGivenXandUtility (L582-L597): y = (ku)² / (a·ku + x) − b·ku. Возвращает: (x, y) или None (откат).
pub fn point_given_x(x: I256, u: I256, s: Slice) -> Option<(I256, I256)> {
    let ku = muli(s.k, u)?;
    let y = ku
        .checked_mul(ku)?
        .checked_div(muli(s.a, ku)?.checked_add(x)?)?
        .checked_sub(muli(s.b, ku)?)?;
    (!y.is_negative()).then_some((x, y))
}

/// _getPointGivenYandUtility (L611-L624): x = (ku)² / (b·ku + y) − a·ku. Возвращает: (x, y) или None (откат).
pub fn point_given_y(y: I256, u: I256, s: Slice) -> Option<(I256, I256)> {
    let ku = muli(s.k, u)?;
    let x = ku
        .checked_mul(ku)?
        .checked_div(muli(s.b, ku)?.checked_add(y)?)?
        .checked_sub(muli(s.a, ku)?)?;
    (!x.is_negative()).then_some((x, y))
}

/// _findSlice (L642-L666): номер среза точки по наклону y/x. None — BalanceError / BoundaryError.
pub fn find_slice(slopes: &[I256], x: I256, y: I256) -> Option<usize> {
    if x < min_balance() || y < min_balance() {
        return None;
    }
    let m = divi(y, x)?;
    if m <= min_m() {
        return None;
    }
    if let Some(i) = slopes.iter().position(|s| m <= *s) {
        return Some(i);
    }
    (m <= max_m()).then_some(slopes.len())
}

/// _getSliceBoundaries (L699-L718): (mLeft, mRight) среза. None — номер вне срезов (BoundaryError).
fn boundaries(slopes: &[I256], i: usize) -> Option<(I256, I256)> {
    let n = slopes.len();
    if n < i {
        return None;
    }
    Some(if i == 0 {
        (*slopes.first()?, min_m())
    } else if i == n {
        (max_m(), slopes[i - 1])
    } else {
        (slopes[i], slopes[i - 1])
    })
}

/// _pointIsNotInSlice (L675-L687): точка вне среза: m ≤ mRight или mLeft < m. None — откат (divi, границы).
fn not_in_slice(slopes: &[I256], i: usize, x: I256, y: I256) -> Option<bool> {
    let (left, right) = boundaries(slopes, i)?;
    let m = divi(y, x)?;
    Some(m <= right || left < m)
}

/// _next (L729-L745): соседний срез по направлению (+1 — влево, −1 — вправо). None — за краем (BoundaryError).
fn next(i: usize, dir: i8, slices: usize) -> Option<usize> {
    let p = i as i64 + i64::from(dir);
    (p >= 0 && (p as usize) < slices).then_some(p as usize)
}

/// Какая координата известна в _findFinalPoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Known {
    X,
    Y,
}

/// _findFinalPoint (L507-L525): от среза i ищет срез, где точка (известная координата, полезность) лежит в нём.
/// Принимает: известную координату и какая она, направление поиска, полезность, начальный срез, наклоны и срезы.
/// Возвращает: (xf, yf) или None (откат).
pub fn final_point(
    fixed: I256,
    known: Known,
    dir: i8,
    u: I256,
    start: usize,
    slopes: &[I256],
    slices: &[Slice],
) -> Option<(I256, I256)> {
    let point = |i: usize| match known {
        Known::X => point_given_x(fixed, u, *slices.get(i)?),
        Known::Y => point_given_y(fixed, u, *slices.get(i)?),
    };
    let mut i = start;
    let (mut xf, mut yf) = point(i)?;
    while not_in_slice(slopes, i, xf, yf)? {
        i = next(i, dir, slices.len())?;
        (xf, yf) = point(i)?;
    }
    Some((xf, yf))
}

//! Файл: venues/bsc_dex/math/wombat_core.rs
//! Что делает: решение 484 — арифметика Wombat Exchange один-в-один с контрактом: DSMath (wmul / wdiv / toWad / fromWad
//! над uint256), SignedSafeMath (wmul / wdiv / sqrt с начальным приближением над int256) и ядро CoreV2 (_swapQuoteFunc,
//! _solveQuad). Solidity 0.8: переполнение, деление на ноль, MIN / −1 — откат, здесь — None. Деление int256 — к нулю
//! (как I256 checked_div). Квадратный корень с приближением: целочисленный Вавилон `while (x != z)` у некоторых y
//! качается между ⌊√y⌋ и ⌊√y⌋ + 1 бесконечно — на цепи вызов исчерпывает газ и откатывается; здесь качание (новое x
//! равно значению до z) — None, и предел итераций MAX_ITER — None.
//! Кто вызывает: math/wombat.rs.
//! От чего зависит: alloy-primitives (U256, I256).
//! Настройки: нет.
//!
//! Источник (развёрнутые реализации Arbitrum совпадают побайтно — docs/sources/wombat_arbitrum/README.md):
//! wombat-exchange/v1-core 2ac067b8 contracts/wombat-core/libraries/DSMath.sol L37-L64 (wmul, wdiv, toWad, fromWad),
//! libraries/SignedSafeMath.sol L16-L23 (wdiv, wmul), L40-L57 (sqrt(y, guess)),
//! pool/CoreV2.sol L31-L64 (_swapQuoteFunc, _solveQuad).

use alloy_primitives::{I256, U256};

/// Предел итераций корня — только защита цикла от зависания: контракт число итераций не ограничивает (его предел —
/// газ транзакции), качание ловится отдельно; это не порог торговли и не факт поведения на цепи.
const MAX_ITER: u32 = 1024;

/// WAD = 10^18 (DSMath.sol L21). Ошибок нет.
pub fn wad() -> U256 {
    U256::from(1_000_000_000_000_000_000u64)
}

/// WAD как int256 (SignedSafeMath.sol L13, CoreV2.sol L15). Ошибок нет.
pub fn wad_i() -> I256 {
    I256::from_raw(wad())
}

/// int256 из малого целого. Ошибок нет.
fn int(n: i64) -> I256 {
    I256::unchecked_from(n)
}

/// DSMath.wmul (L38-L40): (x·y + WAD/2) / WAD. None — переполнение.
pub fn wmul(x: U256, y: U256) -> Option<U256> {
    Some(x.checked_mul(y)?.checked_add(wad() / U256::from(2u8))? / wad())
}

/// DSMath.wdiv (L42-L44): (x·WAD + y/2) / y. None — переполнение, y = 0.
pub fn wdiv(x: U256, y: U256) -> Option<U256> {
    x.checked_mul(wad())?
        .checked_add(y / U256::from(2u8))?
        .checked_div(y)
}

/// DSMath.toWad (L47-L54): из d знаков в 18. None — переполнение.
pub fn to_wad(x: U256, d: u8) -> Option<U256> {
    match d {
        0..=17 => x.checked_mul(ten(18 - d)),
        18 => Some(x),
        _ => Some(x / ten(d - 18)),
    }
}

/// DSMath.fromWad (L57-L64): из 18 знаков в d. None — переполнение.
pub fn from_wad(x: U256, d: u8) -> Option<U256> {
    match d {
        0..=17 => Some(x / ten(18 - d)),
        18 => Some(x),
        _ => x.checked_mul(ten(d - 18)),
    }
}

/// 10^n (n ≤ 77). Ошибок нет.
fn ten(n: u8) -> U256 {
    U256::from(10u8).pow(U256::from(n))
}

/// SignedSafeMath.wmul (L21-L23): (x·y + WAD/2) / WAD, деление к нулю. None — переполнение.
pub fn wmul_i(x: I256, y: I256) -> Option<I256> {
    x.checked_mul(y)?
        .checked_add(wad_i().checked_div(int(2))?)?
        .checked_div(wad_i())
}

/// SignedSafeMath.wdiv (L16-L18): (x·WAD + y/2) / y. None — переполнение, y = 0.
pub fn wdiv_i(x: I256, y: I256) -> Option<I256> {
    x.checked_mul(wad_i())?
        .checked_add(y.checked_div(int(2))?)?
        .checked_div(y)
}

/// SignedSafeMath.sqrt(y, guess) (L40-L57): Вавилон с начальным приближением. None — переполнение (−guess у MIN),
/// качание между двумя значениями или MAX_ITER (на цепи — исчерпание газа, откат).
pub fn sqrt_guess(y: I256, guess: I256) -> Option<I256> {
    if y > int(3) {
        let neg = guess.checked_neg()?;
        let mut z = if guess > I256::ZERO && guess <= y {
            guess
        } else if guess < I256::ZERO && neg <= y {
            neg
        } else {
            y
        };
        let mut x = y.checked_div(z)?.checked_add(z)?.checked_div(int(2))?;
        let mut before = None;
        for _ in 0..MAX_ITER {
            if x == z {
                return Some(z);
            }
            if before == Some(x) {
                return None;
            }
            before = Some(z);
            z = x;
            x = y.checked_div(x)?.checked_add(x)?.checked_div(int(2))?;
        }
        None
    } else if y != I256::ZERO {
        Some(I256::ONE)
    } else {
        Some(I256::ZERO)
    }
}

/// CoreV2._solveQuad (L62-L64): ((b·b + c·4·WAD).sqrt(b) − b) / 2. None — как у операций.
pub fn solve_quad(b: I256, c: I256) -> Option<I256> {
    let under = b
        .checked_mul(b)?
        .checked_add(c.checked_mul(int(4))?.checked_mul(wad_i())?)?;
    sqrt_guess(under, b)?.checked_sub(b)?.checked_div(int(2))
}

/// CoreV2._swapQuoteFunc (L31-L53): выход идеального обмена (без комиссии) по cash / liability монет x (вход) и y
/// (выход), входу dx и усилению a — всё 18 знаков. Возвращает |Dy|. None — Lx = 0 или Ly = 0 (CORE_UNDERFLOW),
/// деление на ноль (Ax = 0, Ay = 0, rx' = 0), переполнение, корень не сошёлся.
pub fn swap_quote(ax: I256, ay: I256, lx: I256, ly: I256, dx: I256, a: I256) -> Option<U256> {
    if lx == I256::ZERO || ly == I256::ZERO {
        return None;
    }
    // D = Ax + Ay − A.wmul((Lx·Lx)/Ax + (Ly·Ly)/Ay) — L43.
    let inner = lx
        .checked_mul(lx)?
        .checked_div(ax)?
        .checked_add(ly.checked_mul(ly)?.checked_div(ay)?)?;
    let d = ax.checked_add(ay)?.checked_sub(wmul_i(a, inner)?)?;
    // rx' = (Ax + Dx).wdiv(Lx) — L44.
    let rx = wdiv_i(ax.checked_add(dx)?, lx)?;
    // b = (Lx·(rx' − A.wdiv(rx'))) / Ly − D.wdiv(Ly) — L45.
    let b = lx
        .checked_mul(rx.checked_sub(wdiv_i(a, rx)?)?)?
        .checked_div(ly)?
        .checked_sub(wdiv_i(d, ly)?)?;
    let ry = solve_quad(b, a)?;
    // Dy = Ly.wmul(ry') − Ay — L47; quote = |Dy| — L48-L52.
    let dy = wmul_i(ly, ry)?.checked_sub(ay)?;
    Some(dy.unsigned_abs())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Корень: точные квадраты сходятся, y = 8 качается 2 ↔ 3 (на цепи — исчерпание газа) — None.
    #[test]
    fn sqrt_converges_or_cycles() {
        assert_eq!(sqrt_guess(int(16), int(16)), Some(int(4)));
        assert_eq!(sqrt_guess(int(1_000_000), int(-7)), Some(int(1000)));
        assert_eq!(sqrt_guess(int(8), int(8)), None);
        assert_eq!(sqrt_guess(int(3), int(0)), Some(I256::ONE));
        assert_eq!(sqrt_guess(int(0), int(0)), Some(I256::ZERO));
    }

    /// Деление int256 — к нулю: wmul(−1, 1) = (−1 + 5·10^17) / 10^18 = 0.
    #[test]
    fn signed_rounding_toward_zero() {
        assert_eq!(wmul_i(int(-1), int(1)), Some(I256::ZERO));
        assert_eq!(wdiv_i(int(-3), int(2)), Some(int(-1_499_999_999_999_999_999)));
    }
}

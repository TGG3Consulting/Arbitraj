//! Файл: venues/bsc_dex/math/crypto_newton3.rs
//! Что делает: запасной метод Ньютона `_newton_y` математики Tricrypto-NG (решение 442) — им get_y считает y, когда
//! дискриминант кубического уравнения не положителен; сортировка трёх чисел `_sort`; проверка доли монеты `frac`.
//! Все операции uint256 проверяемые (Vyper): переполнение, деление на ноль, assert и «Did not converge» — None.
//! Кто вызывает: math/crypto_y3.rs, math/crypto_d3.rs (sort).
//! От чего зависит: math/crypto_ops.rs, math/crypto_y2.rs (mul1 — та же формула), math/ten.rs.
//! Настройки: нет.
//!
//! Источник: curvefi/tricrypto-ng ecaa816, contracts/main/CurveCryptoMathOptimized3.vy: `_newton_y` L236-L333,
//! `_sort` L845-L865.

use alloy_primitives::U256;

use super::crypto_ops::{au, du, mu, su};
use super::crypto_y2::mul1;
use super::ten::{self, E18};

/// N_COINS (L18).
const N: u64 = 3;

/// Доля монеты frac = x·10^18/D в пределах assert'а `frac > 10**16 - 1 and frac < 10**20 + 1` (L56, L248, L329).
/// Ошибок нет.
pub fn frac_ok(frac: U256) -> bool {
    (ten::pow(16)..=ten::pow(20)).contains(&frac)
}

/// `_sort` (L845-L865): три числа по убыванию теми же обменами, что в исходнике. Ошибок нет.
pub fn sort(mut x: [U256; 3]) -> [U256; 3] {
    if x[0] < x[1] {
        x.swap(0, 1);
    }
    if x[0] < x[2] {
        x.swap(0, 2);
    }
    if x[1] < x[2] {
        x.swap(1, 2);
    }
    x
}

/// `_newton_y(ANN, gamma, x, D, i)` (L236-L333). Принимает: A, gamma, балансы в 18 знаках и ценах пула, D, индекс.
/// Возвращает: y или None (откат контракта).
pub fn newton_y(ann: U256, gamma: U256, x: [U256; 3], d: U256, idx: usize) -> Option<U256> {
    let n = U256::from(N);
    let one = U256::from(1u8);
    for (k, xk) in x.iter().enumerate() {
        if k != idx && !frac_ok(du(mu(*xk, E18)?, d)?) {
            return None;
        }
    }
    let mut y = d / n;
    let mut k0_i = E18;
    let mut s_i = U256::ZERO;
    let mut sorted = x;
    sorted[idx] = U256::ZERO;
    let sorted = sort(sorted);
    let e14 = ten::pow(14);
    let conv = (sorted[0] / e14).max(d / e14).max(U256::from(100u8));
    // Малые x первыми (L262-L265).
    for xx in sorted[..N as usize - 1].iter().rev() {
        y = du(mu(y, d)?, mu(*xx, n)?)?;
        s_i = au(s_i, *xx)?;
    }
    // Большие x первыми (L267-L268).
    for xx in sorted.iter().take(N as usize - 1) {
        k0_i = du(mu(mu(k0_i, *xx)?, n)?, d)?;
    }
    for _ in 0..255 {
        let y_prev = y;
        let k0 = du(mu(mu(k0_i, y)?, n)?, d)?;
        let s = au(s_i, y)?;
        let g = au(gamma, E18)?;
        let g1k0 = if g > k0 { g - k0 + one } else { k0 - g + one };
        let mul1 = mul1(d, gamma, g1k0, ann)?;
        let mul2 = au(E18, du(mu(U256::from(2u8) * E18, k0)?, g1k0)?)?;
        let mut yfprime = au(au(mu(E18, y)?, mu(s, mul2)?)?, mul1)?;
        let dyfprime = mu(d, mul2)?;
        if yfprime < dyfprime {
            y = y_prev / U256::from(2u8);
            continue;
        }
        yfprime -= dyfprime;
        let fprime = du(yfprime, y)?;
        let mut y_minus = du(mul1, fprime)?;
        let plus_a = du(au(yfprime, mu(E18, d)?)?, fprime)?;
        let y_plus = au(plus_a, du(mu(y_minus, E18)?, k0)?)?;
        y_minus = au(y_minus, du(mu(E18, s)?, fprime)?)?;
        y = if y_plus < y_minus {
            y_prev / U256::from(2u8)
        } else {
            su(y_plus, y_minus)?
        };
        let diff = if y > y_prev { y - y_prev } else { y_prev - y };
        if diff < conv.max(y / e14) {
            return frac_ok(du(mu(y, E18)?, d)?).then_some(y);
        }
    }
    None
}

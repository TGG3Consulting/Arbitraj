//! Файл: venues/bsc_dex/math/crypto_y2.rs
//! Что делает: `get_y` математики Twocrypto-NG (решение 442) — баланс монеты i (в 18 знаках и ценах пула) при балансе
//! другой монеты и инварианте D: аналитическое решение кубического уравнения, при неположительном дискриминанте —
//! запасной метод Ньютона `_newton_y`. Возвращает [y, K0_prev] как контракт (второе — только начальное приближение
//! newton_D в tweak_price; котировке не нужно, но его convert тоже может откатить обмен). Арифметика — как Vyper
//! (math/crypto_ops.rs): где контракт откатился бы (assert, переполнение, деление на ноль, «Did not converge») — None.
//! Кто вызывает: math/crypto_swap.rs.
//! От чего зависит: math/crypto_ops.rs, math/ten.rs.
//! Настройки: нет.
//!
//! Источник: curvefi/twocrypto-ng 89c8dfb (тег prod-v2.1.0; математика та же, что на коммите 99b5192 «updated arbitrum
//! amm and math implementation» — git diff пуст; развёрнута в Arbitrum One: deployments.yaml `arbitrum:mainnet math`
//! 0x1Fd8Af16DC4BEBd950521308D55d0543b6cDF4A1), contracts/main/CurveCryptoMathOptimized2.vy: константы L17-L27,
//! `_newton_y` L143-L206, `get_y` L229-L379. Решение 453: та же формула у v2.0.0 (d612fde, `get_y` L225-L364) — версии
//! отличаются только пределами assert'ов (math/crypto_lim2.rs).

use alloy_primitives::{I256, U256};

use crate::core::types::CryptoMath;

use super::crypto_lim2::{self as lim2, Lim};
use super::crypto_ops::{
    abs, add, au, cbrt, div, du, i, ie, isqrt, iu, mu, mul, sq, su, sub, to_i, to_u, udiv,
    udiv_u, umul,
};
use super::ten::{self, E18};

/// N_COINS (L17).
const N: u64 = 2;
/// A_MULTIPLIER (L18).
const A_MULTIPLIER: u64 = 10_000;

/// `get_y(ANN, gamma, x, D, i)` (L229-L379). Принимает: версию математики (решение 453: TwoNg210 / TwoNg200 — пределы
/// assert'ов, math/crypto_lim2.rs), A (как отдаёт пул), gamma, балансы в 18 знаках и ценах пула, D, индекс искомой
/// монеты (0 или 1). Возвращает: [y, K0_prev] или None (откат контракта).
pub fn get_y(
    m: CryptoMath,
    ann_u: U256,
    gamma_u: U256,
    x: [U256; 2],
    d_u: U256,
    idx: usize,
) -> Option<[U256; 2]> {
    let l = lim2::of(m, gamma_u);
    if idx > 1 || !lim2::a_gamma_ok(&l, ann_u, gamma_u) || !lim2::d_ok(d_u) {
        return None;
    }
    let ann = to_i(ann_u)?;
    let gamma = to_i(gamma_u)?;
    let d = to_i(d_u)?;
    let x_j = to_i(x[1 - idx])?;
    let gamma2 = umul(gamma, gamma);
    // y = D**2 / (x_j * N_COINS**2) (L253) — проверяемые; значение дальше не нужно (как в исходнике).
    let _y0 = div(sq(d)?, mul(x_j, iu(N * N))?)?;
    // K0_i = unsafe_div(10**18 * N_COINS * x_j, D) (L256), assert в пределах lim_mul (L257).
    let k0_i = udiv(mul(i(E18 * U256::from(N)), x_j)?, d);
    if !(to_i(l.k0.0)?..=to_i(l.k0.1)?).contains(&k0_i) {
        return None;
    }
    let ann_gamma2 = mul(ann, gamma2)?;
    let three = iu(3);
    let mut a = ie(32);
    let mut b = b_coef(d, ann_gamma2, x_j, gamma)?;
    let mut c = c_coef(d, ann_gamma2, x_j, gamma, gamma2)?;
    // d = -unsafe_div(unsafe_add(10**18, gamma)**2, 10**4) (L282).
    let mut dd = neg(udiv(sq(gamma.wrapping_add(ie(18)))?, ie(4)))?;
    // delta0 = 3*a*c/b - b (L285); delta1 = 3*delta0 + b - 27*a**2/b*d/b (L288).
    let delta0 = sub(div(mul(mul(three, a)?, c)?, b)?, b)?;
    let t = div(mul(div(mul(iu(27), sq(a)?)?, b)?, dd)?, b)?;
    let delta1 = sub(add(mul(three, delta0)?, b)?, t)?;
    let threshold = abs(delta0)?.min(abs(delta1)?).min(a);
    let divider = divider(threshold);
    a = udiv(a, divider);
    b = udiv(b, divider);
    c = udiv(c, divider);
    dd = udiv(dd, divider);
    // Пересчёт после деления (L327-L331).
    let delta0 = sub(udiv(umul(umul(three, a), c), b), b)?;
    let t = udiv(umul(udiv(umul(iu(27), sq(a)?), b), dd), b);
    let delta1 = sub(add(mul(three, delta0)?, b)?, t)?;
    // sqrt_arg = delta1**2 + unsafe_mul(unsafe_div(4*delta0**2, b), delta0) (L334).
    let tail = umul(udiv(mul(iu(4), sq(delta0)?)?, b), delta0);
    let sqrt_arg = add(sq(delta1)?, tail)?;
    if sqrt_arg <= I256::ZERO {
        let y = newton_y(ann_u, gamma_u, x, d_u, idx, &l)?;
        return Some([y, U256::ZERO]);
    }
    let sqrt_val = to_i(isqrt(to_u(sqrt_arg)?))?;
    let root = root(a, b, delta0, delta1, sqrt_val)?;
    // y = D**2/x_j*root/4/10**18 (L368) — unsafe, кроме D**2 и convert.
    let y = to_u(udiv(udiv(umul(udiv(sq(d)?, x_j), root), iu(4)), ie(18)))?;
    let k0 = to_u(root)?;
    // frac = unsafe_div(y*10**18, D) в пределах (L370-L371).
    let frac = udiv_u(mu(y, E18)?, d_u);
    if !lim2::within(frac, l.frac_y) {
        return None;
    }
    Some([y, k0])
}

/// b = D*ann_gamma2/400000000/x_j − 3·10^32 − 2·gamma·10^14 (L265-L269). None — откат.
fn b_coef(d: I256, ann_gamma2: I256, x_j: I256, gamma: I256) -> Option<I256> {
    let first = div(div(mul(d, ann_gamma2)?, iu(400_000_000))?, x_j)?;
    let second = umul(ie(32), iu(3));
    let third = umul(umul(iu(2), gamma), ie(14));
    sub(sub(first, second)?, third)
}

/// c = 3·10^32 + 4·gamma·10^14 + gamma2/10^4 + (4·ann_gamma2/4·10^8)·x_j/D − 4·ann_gamma2/4·10^8 (L272-L278).
/// None — откат.
fn c_coef(d: I256, ann_gamma2: I256, x_j: I256, gamma: I256, gamma2: I256) -> Option<I256> {
    let q = udiv(umul(iu(4), ann_gamma2), iu(400_000_000));
    let mut c = add(umul(ie(32), iu(3)), umul(umul(iu(4), gamma), ie(14)))?;
    c = add(c, udiv(gamma2, ie(4)))?;
    c = add(c, udiv(mul(q, x_j)?, d))?;
    sub(c, q)
}

/// Корень кубического уравнения (L341-L365): кубические корни b и (delta1 ± sqrt_val)/2, C1, root. None — откат.
fn root(a: I256, b: I256, delta0: I256, delta1: I256, sqrt_val: I256) -> Option<I256> {
    let two = U256::from(2u8);
    let b_cbrt = if b > I256::ZERO {
        to_i(cbrt(to_u(b)?))?
    } else {
        neg(to_i(cbrt(to_u(neg(b)?)?))?)?
    };
    let second_cbrt = if delta1 > I256::ZERO {
        to_i(cbrt(du(to_u(delta1.wrapping_add(sqrt_val))?, two)?))?
    } else {
        let v = to_u(sqrt_val.wrapping_sub(delta1))?;
        neg(to_i(cbrt(udiv_u(v, two)))?)?
    };
    // C1 = unsafe_div(unsafe_mul(unsafe_div(b_cbrt**2, 10**18), second_cbrt), 10**18) (L360).
    let c1 = udiv(umul(udiv(sq(b_cbrt)?, ie(18)), second_cbrt), ie(18));
    // root = (10**18*C1 - 10**18*b - 10**18*b/C1*delta0) / (3*a) (L363).
    let e18 = ie(18);
    let part = mul(div(umul(e18, b), c1)?, delta0)?;
    let r = sub(sub(umul(e18, c1), umul(e18, b))?, part)?;
    div(r, umul(iu(3), a))
}

/// Унарный минус int256 (проверяемый). None — переполнение.
fn neg(v: I256) -> Option<I256> {
    I256::ZERO.checked_sub(v)
}

/// Делитель по порогу (L292-L320): цепочка elif исходника. Ошибок нет.
fn divider(threshold: I256) -> I256 {
    // Пороги 10^p и делители 10^q по убыванию p.
    const P: [u64; 14] = [48, 46, 44, 42, 40, 38, 36, 34, 32, 30, 28, 26, 24, 20];
    const Q: [u64; 14] = [30, 28, 26, 24, 22, 20, 18, 16, 14, 12, 10, 8, 6, 2];
    for (p, q) in P.into_iter().zip(Q) {
        if threshold > ie(p) {
            return ie(q);
        }
    }
    iu(1)
}

/// `_newton_y(ANN, gamma, x, D, i, lim_mul)` (L143-L206; v2.0.0 — без lim_mul, d612fde L142-L204) — запасной метод
/// Ньютона (uint256, все операции проверяемые). Принимает: пределы версии (K0_i). Возвращает: y или None (откат:
/// assert K0_i, переполнение, деление на ноль, нет сходимости за 255 раундов).
fn newton_y(ann: U256, gamma: U256, x: [U256; 2], d: U256, idx: usize, l: &Lim) -> Option<U256> {
    let n = U256::from(N);
    let one = U256::from(1u8);
    let x_j = x[1 - idx];
    let mut y = du(mu(d, d)?, mu(x_j, n * n)?)?;
    let k0_i = du(mu(E18 * n, x_j)?, d)?;
    if !lim2::within(k0_i, l.k0) {
        return None;
    }
    let e14 = ten::pow(14);
    let conv = (x_j / e14).max(d / e14).max(U256::from(100u8));
    for _ in 0..255 {
        let y_prev = y;
        let k0 = du(mu(mu(k0_i, y)?, n)?, d)?;
        let s = au(x_j, y)?;
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
            return Some(y);
        }
    }
    None
}

/// mul1 = 10**18 * D / gamma * _g1k0 / gamma * _g1k0 * A_MULTIPLIER / ANN (L176) — общий для Twocrypto и
/// Tricrypto (CurveCryptoMathOptimized3.vy L298). None — откат.
pub fn mul1(d: U256, gamma: U256, g1k0: U256, ann: U256) -> Option<U256> {
    let v = du(mu(E18, d)?, gamma)?;
    let v = du(mu(v, g1k0)?, gamma)?;
    du(mu(mu(v, g1k0)?, U256::from(A_MULTIPLIER))?, ann)
}

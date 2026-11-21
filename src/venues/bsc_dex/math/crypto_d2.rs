//! Файл: venues/bsc_dex/math/crypto_d2.rs
//! Что делает: `newton_D` математики Twocrypto-NG (решение 453) — инвариант D по балансам (в 18 знаках и ценах пула)
//! методом Ньютона. Нужен в рампе A/gamma: `_exchange` пула пересчитывает D по балансам ДО обмена (K0_prev = 0) и
//! только потом зовёт get_y. Арифметика Vyper 0.3.10 (math/crypto_ops.rs): проверяемые операции — откат → None,
//! unsafe_* — по модулю 2^256. Версии v2.1.0 / v2.0.0 — пределы assert'ов из math/crypto_lim2.rs.
//! Кто вызывает: math/crypto_swap.rs.
//! От чего зависит: core/types (CryptoMath), math/crypto_lim2.rs, math/crypto_ops.rs, math/ten.rs.
//! Настройки: нет.
//!
//! Источник: curvefi/twocrypto-ng 89c8dfb, contracts/main/CurveCryptoMathOptimized2.vy `newton_D` L376-L460 (ветка
//! K0_prev == 0 — L398-L399, решение 472 — K0_prev ≠ 0, L400-L404); v2.0.0 — d612fde L368-L452 (та же формула, другие MAX_GAMMA и frac L449). Вызов в рампе —
//! CurveTwocryptoOptimized.vy `_exchange` L770-L781 (89c8dfb; у d612fde — тот же текст).

use alloy_primitives::U256;

use crate::core::types::CryptoMath;

use super::crypto_lim2 as lim2;
use super::crypto_ops::{au, du, isqrt, mu, su, udiv_u};
use super::ten::{self, E18};

/// N_COINS (L17).
const N: u64 = 2;
/// A_MULTIPLIER (L18).
const A_MULTIPLIER: u64 = 10_000;

/// `newton_D(ANN, gamma, x_unsorted, 0)`. Принимает: версию, A (как отдаёт пул), gamma, балансы в 18 знаках и ценах
/// пула. Возвращает: D или None (откат: assert'ы, переполнение, деление на ноль, «Did not converge»).
pub fn newton_d(m: CryptoMath, ann: U256, gamma: U256, xu: [U256; 2]) -> Option<U256> {
    newton_d_k0(m, ann, gamma, xu, U256::ZERO)
}

/// Решение 472: `newton_D(ANN, gamma, x_unsorted, K0_prev)` — так зовёт tweak_price после обмена (K0_prev — второе
/// значение get_y). K0_prev ≠ 0: начальное D = isqrt(unsafe_mul(unsafe_div(4·x0·x1 (unsafe), K0_prev), 10^18)), не
/// больше S (L400-L404). Принимает: как newton_d и K0_prev. Возвращает: D или None (откат).
pub fn newton_d_k0(
    m: CryptoMath,
    ann: U256,
    gamma: U256,
    xu: [U256; 2],
    k0_prev: U256,
) -> Option<U256> {
    let l = lim2::of(m, gamma);
    if !lim2::a_gamma_ok(&l, ann, gamma) {
        return None;
    }
    let x = if xu[0] < xu[1] { [xu[1], xu[0]] } else { xu };
    // assert x[0] > 10**9 - 1 and x[0] < 10**15 * 10**18 + 1 (L392).
    if !(ten::pow(9)..=ten::pow(33)).contains(&x[0]) {
        return None;
    }
    // assert unsafe_div(x[1] * 10**18, x[0]) > 10**14 - 1 (L393).
    if udiv_u(mu(x[1], E18)?, x[0]) < ten::pow(14) {
        return None;
    }
    let s = x[0].wrapping_add(x[1]);
    let n = U256::from(N);
    // D = N_COINS * isqrt(unsafe_mul(x[0], x[1])) (L399); при K0_prev — L402-L404.
    let mut d = if k0_prev.is_zero() {
        mu(n, isqrt(x[0].wrapping_mul(x[1])))?
    } else {
        let p = U256::from(4u8).wrapping_mul(x[0]).wrapping_mul(x[1]);
        isqrt(udiv_u(p, k0_prev).wrapping_mul(E18)).min(s)
    };
    let g1k0_base = au(gamma, E18)?;
    let one = U256::from(1u8);
    for _ in 0..255 {
        let d_prev = d;
        if d.is_zero() {
            return None;
        }
        // K0 = unsafe_div(unsafe_div((10**18 * N_COINS**2) * x[0], D) * x[1], D) (L418).
        let k0 = udiv_u(mu(udiv_u(mu(E18 * n * n, x[0])?, d), x[1])?, d);
        let g1k0 = if g1k0_base > k0 {
            g1k0_base.wrapping_sub(k0).wrapping_add(one)
        } else {
            k0.wrapping_sub(g1k0_base).wrapping_add(one)
        };
        // mul1 (L427): unsafe_div(unsafe_div(unsafe_div(10**18 * D, gamma) * _g1k0, gamma) * _g1k0
        // * A_MULTIPLIER, ANN).
        let t = mu(udiv_u(mu(E18, d)?, gamma), g1k0)?;
        let t = mu(mu(udiv_u(t, gamma), g1k0)?, U256::from(A_MULTIPLIER))?;
        let mul1 = udiv_u(t, ann);
        // mul2 = unsafe_div(((2 * 10**18) * N_COINS) * K0, _g1k0) (L430).
        let mul2 = udiv_u(mu(U256::from(2u8) * E18 * n, k0)?, g1k0);
        // neg_fprime = (S + unsafe_div(S * mul2, 10**18)) + mul1 * N_COINS / K0
        // - unsafe_div(mul2 * D, 10**18) (L433).
        let a = au(s, udiv_u(mu(s, mul2)?, E18))?;
        let a = au(a, du(mu(mul1, n)?, k0)?)?;
        let neg_fprime = su(a, udiv_u(mu(mul2, d)?, E18))?;
        // D_plus = D * (neg_fprime + S) / neg_fprime; D_minus = unsafe_div(D * D, neg_fprime) (L436-L437).
        let d_plus = du(mu(d, au(neg_fprime, s)?)?, neg_fprime)?;
        let mut d_minus = udiv_u(mu(d, d)?, neg_fprime);
        let part = |delta: U256| -> Option<U256> {
            let v = udiv_u(mu(d, udiv_u(mul1, neg_fprime))?, E18);
            Some(udiv_u(mu(v, delta)?, k0))
        };
        // L438-L441.
        if E18 > k0 {
            d_minus = au(d_minus, part(E18.wrapping_sub(k0))?)?;
        } else {
            d_minus = su(d_minus, part(k0.wrapping_sub(E18))?)?;
        }
        d = if d_plus > d_minus {
            d_plus.wrapping_sub(d_minus)
        } else {
            udiv_u(d_minus.wrapping_sub(d_plus), U256::from(2u8))
        };
        let diff = if d > d_prev {
            d.wrapping_sub(d_prev)
        } else {
            d_prev.wrapping_sub(d)
        };
        // if diff * 10**14 < max(10**16, D) (L453): frac каждой монеты в пределах версии, затем D.
        if mu(diff, ten::pow(14))? < d.max(ten::pow(16)) {
            for xk in x {
                let frac = du(mu(xk, E18)?, d)?;
                if !lim2::within(frac, l.frac_x) {
                    return None;
                }
            }
            return Some(d);
        }
    }
    None
}

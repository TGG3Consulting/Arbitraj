//! Файл: venues/bsc_dex/math/crypto_d3.rs
//! Что делает: `newton_D` математики Tricrypto-NG (решение 453) — инвариант D по трём балансам (в 18 знаках и ценах
//! пула) методом Ньютона, начальное приближение — 3·`_geometric_mean` (K0_prev = 0). Нужен в рампе A/gamma: `_exchange`
//! пула пересчитывает D по балансам ДО обмена и только потом зовёт get_y. Арифметика Vyper 0.3.10
//! (math/crypto_ops.rs): почти всё — unsafe_* (по модулю 2^256, деление на ноль — 0), проверяемые — как в исходнике.
//! Кто вызывает: math/crypto_swap.rs.
//! От чего зависит: math/crypto_newton3.rs (sort), math/crypto_ops.rs, math/ten.rs.
//! Настройки: нет.
//!
//! Источник: curvefi/tricrypto-ng ecaa816, contracts/main/CurveCryptoMathOptimized3.vy: `newton_D` L337-L534 (ветка
//! K0_prev == 0 — L362-L364), `_geometric_mean` L868-L880, `_cbrt` L789-L842 (math/crypto_ops.rs). Вызов в рампе —
//! CurveTricryptoOptimizedWETH.vy `_exchange` L901-L913. Математика, развёрнутая в Arbitrum One (0x604388Bb…), — коммит
//! 2d03e6f, отличие от ecaa816 только в pragma (решение 442).

use alloy_primitives::U256;

use super::crypto_newton3::sort;
use super::crypto_ops::{au, cbrt, mu, su, udiv_u};
use super::ten::{self, E18};

/// N_COINS (L18).
const N: u64 = 3;
/// A_MULTIPLIER (L19).
const A_MULTIPLIER: u64 = 10_000;

/// `_geometric_mean(_x)` (L868-L880): prod = unsafe_div(unsafe_div(x0 * x1, 10**18) * x2, 10**18) (умножения
/// проверяемые), 0 при prod = 0, иначе _cbrt(prod). Возвращает: среднее или None (переполнение умножения).
fn geometric_mean(x: [U256; 3]) -> Option<U256> {
    let prod = udiv_u(mu(udiv_u(mu(x[0], x[1])?, E18), x[2])?, E18);
    if prod.is_zero() {
        return Some(U256::ZERO);
    }
    Some(cbrt(prod))
}

/// `newton_D(ANN, gamma, x_unsorted, 0)` (L337-L534). Принимает: A (как отдаёт пул), gamma, балансы в 18 знаках и
/// ценах пула. Возвращает: D или None (откат: assert'ы L354-L355 и L531, переполнение проверяемых операций,
/// «Did not converge»).
pub fn newton_d(ann: U256, gamma: U256, xu: [U256; 3]) -> Option<U256> {
    let x = sort(xu);
    // assert x[0] < max_value(uint256) / 10**18 * N_COINS**N_COINS; assert x[0] > 0 (L354-L355).
    let cap = (U256::MAX / E18).wrapping_mul(U256::from(N * N * N));
    if x[0] >= cap || x[0].is_zero() {
        return None;
    }
    let n = U256::from(N);
    let s = x[0].wrapping_add(x[1]).wrapping_add(x[2]);
    let mut d = n.wrapping_mul(geometric_mean(x)?);
    let one = U256::from(1u8);
    for _ in 0..255 {
        let d_prev = d;
        // K0 (L408-L431) — целиком unsafe: ((((10**18·x0·N / D)·x1·N / D)·x2·N) / D.
        let k = udiv_u(E18.wrapping_mul(x[0]).wrapping_mul(n), d);
        let k = udiv_u(k.wrapping_mul(x[1]).wrapping_mul(n), d);
        let k0 = udiv_u(k.wrapping_mul(x[2]).wrapping_mul(n), d);
        let g = gamma.wrapping_add(E18);
        let g1k0 = if g > k0 {
            g.wrapping_sub(k0).wrapping_add(one)
        } else {
            k0.wrapping_sub(g).wrapping_add(one)
        };
        // mul1 (L446-L460) — unsafe: 10**18·D / gamma · _g1k0 / gamma · _g1k0 · A_MULTIPLIER / ANN.
        let t = udiv_u(E18.wrapping_mul(d), gamma).wrapping_mul(g1k0);
        let t = udiv_u(t, gamma).wrapping_mul(g1k0);
        let mul1 = udiv_u(t.wrapping_mul(U256::from(A_MULTIPLIER)), ann);
        // mul2 = unsafe_div(unsafe_mul(2 * 10**18 * N_COINS, K0), _g1k0) (L466-L468).
        let mul2 = udiv_u((U256::from(2u8) * E18 * n).wrapping_mul(k0), g1k0);
        // neg_fprime (L473-L479) — unsafe: S + S·mul2/10**18 + mul1·N/K0 − mul2·D/10**18.
        let a = s.wrapping_add(udiv_u(s.wrapping_mul(mul2), E18));
        let a = a.wrapping_add(udiv_u(mul1.wrapping_mul(n), k0));
        let neg_fprime = a.wrapping_sub(udiv_u(mul2.wrapping_mul(d), E18));
        // D_plus = unsafe_div(D * unsafe_add(neg_fprime, S), neg_fprime) (L488) — D * … проверяемое.
        let d_plus = udiv_u(mu(d, neg_fprime.wrapping_add(s))?, neg_fprime);
        // D_minus = unsafe_div(D * D, neg_fprime) (L491).
        let mut d_minus = udiv_u(mu(d, d)?, neg_fprime);
        let part = |delta: U256| -> Option<U256> {
            let v = udiv_u(mu(d, udiv_u(mul1, neg_fprime))?, E18);
            Some(udiv_u(v.wrapping_mul(delta), k0))
        };
        // L496-L513: D_minus += / -= — проверяемые.
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
        // if unsafe_mul(diff, 10**14) < max(10**16, D) (L526): frac = unsafe_div(unsafe_mul(_x, 10**18), D),
        // assert frac >= 10**16 - 1 and frac < 10**20 + 1 (L531).
        if diff.wrapping_mul(ten::pow(14)) < d.max(ten::pow(16)) {
            let lo = ten::pow(16) - one;
            for xk in x {
                let frac = udiv_u(xk.wrapping_mul(E18), d);
                if frac < lo || frac > ten::pow(20) {
                    return None;
                }
            }
            return Some(d);
        }
    }
    None
}

//! Файл: venues/bsc_dex/math/crypto_v1_2.rs
//! Что делает: математика двухмонетного crypto-пула Curve первого поколения — фабрика crypto v1 (решение 453, пул
//! CurveCryptoSwap2ETH.vy, Vyper 0.3.1; математика — внутри пула): `geometric_mean`, `newton_D`, `newton_y`. Все
//! операции uint256 проверяемые (в исходнике нет unsafe_*): переполнение, деление на ноль, assert, «Did not converge» —
//! None. Цикл Ньютона newton_y общий с трёхмонетным v1 (y_loop — тот же текст CurveCryptoMath3.vy L202-L248).
//! Кто вызывает: math/crypto_swap.rs, math/crypto_v1_3.rs (y_loop).
//! От чего зависит: math/crypto_ops.rs, math/crypto_y2.rs (mul1 — та же формула), math/ten.rs.
//! Настройки: нет (числа — константы исходника).
//!
//! Источник: curvefi/curve-factory-crypto 7ed1011 (пул-шаблон фабрики Arbitrum 0x5bcA7dDF1bcccB2eE8e46c56bfc9d3CDC77262bC,
//! фабрика 0xE5De15A9C9bBedb4F5EC13B131E61245f2983A69 — deployment-logs/others.log; последняя правка шаблона — 186c191),
//! contracts/CurveCryptoSwap2ETH.vy: константы L95-L105, `geometric_mean` L246-L269, `newton_D` L274-L345,
//! `newton_y` L350-L423.

use alloy_primitives::U256;

use super::crypto_ops::{au, du, mu, su};
use super::crypto_y2::mul1;
use super::ten::{self, E18};

/// N_COINS (L103).
const N: u64 = 2;
/// A_MULTIPLIER (L105).
const A_MULTIPLIER: u64 = 10_000;

/// assert'ы A и gamma (L283-L284, L356-L357): ANN ∈ [N^N·A_MULTIPLIER/10; N^N·A_MULTIPLIER·100000] (L98-L99), gamma ∈
/// [10^10; 2·10^16] (L95-L96). Ошибок нет (false — откат).
fn a_gamma_ok(ann: U256, gamma: U256) -> bool {
    let nn = N * N;
    let a = (U256::from(nn * A_MULTIPLIER / 10)..=U256::from(nn * A_MULTIPLIER * 100_000)).contains(&ann);
    a && (ten::E10..=U256::from(2u8) * ten::pow(16)).contains(&gamma)
}

/// Доля frac = v·10^18/D в пределах `frac > 10**16 - 1 and frac < 10**20 + 1`. None — деление на ноль / переполнение;
/// Some(false) — вне пределов.
pub fn frac_ok(v: U256, d: U256) -> Option<bool> {
    let f = du(mu(v, E18)?, d)?;
    Some((ten::pow(16)..=ten::pow(20)).contains(&f))
}

/// `geometric_mean(x, False)` двух чисел (L246-L269): D = x0; D = (D + x0·x1/D) / N до diff ≤ 1 или diff·10^18 < D.
/// Принимает: x по убыванию. Возвращает: D или None (откат).
fn geometric_mean(x: [U256; 2]) -> Option<U256> {
    let n = U256::from(N);
    let mut d = x[0];
    for _ in 0..255 {
        let d_prev = d;
        d = du(au(d, du(mu(x[0], x[1])?, d)?)?, n)?;
        let diff = if d > d_prev { d - d_prev } else { d_prev - d };
        if diff <= U256::from(1u8) || mu(diff, E18)? < d {
            return Some(d);
        }
    }
    None
}

/// `geometric_mean(x, True)` (L246-L269; решение 472 — tweak_price, get_xcp): сначала по убыванию (`if sort and x[0] <
/// x[1]`), затем тот же цикл. Принимает: x в любом порядке. Возвращает: D или None (откат).
pub fn geometric_mean_sorted(x: [U256; 2]) -> Option<U256> {
    geometric_mean(if x[0] < x[1] { [x[1], x[0]] } else { x })
}

/// `newton_D(ANN, gamma, x_unsorted)` (L274-L345). Принимает: A, gamma, балансы в 18 знаках и цене пула. Возвращает: D
/// или None (откат).
pub fn newton_d(ann: U256, gamma: U256, xu: [U256; 2]) -> Option<U256> {
    if !a_gamma_ok(ann, gamma) {
        return None;
    }
    let x = if xu[0] < xu[1] { [xu[1], xu[0]] } else { xu };
    // assert x[0] > 10**9 - 1 and x[0] < 10**15 * 10**18 + 1; assert x[1] * 10**18 / x[0] > 10**14-1 (L291-L292).
    if !(ten::pow(9)..=ten::pow(33)).contains(&x[0]) || du(mu(x[1], E18)?, x[0])? < ten::pow(14) {
        return None;
    }
    let n = U256::from(N);
    let mut d = mu(n, geometric_mean(x)?)?;
    let s = au(x[0], x[1])?;
    let one = U256::from(1u8);
    for _ in 0..255 {
        let d_prev = d;
        // K0 = (10**18 * N_COINS**2) * x[0] / D * x[1] / D (L304).
        let k0 = du(mu(du(mu(E18 * n * n, x[0])?, d)?, x[1])?, d)?;
        let g = au(gamma, E18)?;
        let g1k0 = if g > k0 { au(su(g, k0)?, one)? } else { au(su(k0, g)?, one)? };
        let m1 = mul1(d, gamma, g1k0, ann)?;
        // mul2 = (2 * 10**18) * N_COINS * K0 / _g1k0 (L316).
        let mul2 = du(mu(U256::from(2u8) * E18 * n, k0)?, g1k0)?;
        // neg_fprime = (S + S * mul2 / 10**18) + mul1 * N_COINS / K0 - mul2 * D / 10**18 (L318).
        let a = au(au(s, du(mu(s, mul2)?, E18)?)?, du(mu(m1, n)?, k0)?)?;
        let neg_fprime = su(a, du(mu(mul2, d)?, E18)?)?;
        let d_plus = du(mu(d, au(neg_fprime, s)?)?, neg_fprime)?;
        let mut d_minus = du(mu(d, d)?, neg_fprime)?;
        // D * (mul1 / neg_fprime) / 10**18 * (10**18 - K0) / K0 (L323-L326).
        let part = |delta: U256| du(mu(du(mu(d, du(m1, neg_fprime)?)?, E18)?, delta)?, k0);
        d_minus = if E18 > k0 {
            au(d_minus, part(E18 - k0)?)?
        } else {
            su(d_minus, part(k0 - E18)?)?
        };
        d = if d_plus > d_minus {
            d_plus - d_minus
        } else {
            (d_minus - d_plus) / U256::from(2u8)
        };
        let diff = if d > d_prev { d - d_prev } else { d_prev - d };
        if mu(diff, ten::pow(14))? < d.max(ten::pow(16)) {
            for xk in x {
                if !frac_ok(xk, d)? {
                    return None;
                }
            }
            return Some(d);
        }
    }
    None
}

/// `newton_y(ANN, gamma, x, D, i)` (L350-L423). Принимает: A, gamma, балансы в 18 знаках и цене пула, D, индекс
/// искомой монеты. Возвращает: y или None (откат).
pub fn newton_y(ann: U256, gamma: U256, x: [U256; 2], d: U256, i: usize) -> Option<U256> {
    if i > 1 || !a_gamma_ok(ann, gamma) || !(ten::E17..=ten::pow(33)).contains(&d) {
        return None;
    }
    let n = U256::from(N);
    let x_j = x[1 - i];
    let y = du(mu(d, d)?, mu(x_j, n * n)?)?;
    let k0_i = du(mu(E18 * n, x_j)?, d)?;
    // assert (K0_i > 10**16*N_COINS - 1) and (K0_i < 10**20*N_COINS + 1) (L366).
    if !(ten::pow(16) * n..=ten::pow(20) * n).contains(&k0_i) {
        return None;
    }
    let e14 = ten::pow(14);
    let conv = (x_j / e14).max(d / e14).max(U256::from(100u8));
    y_loop(ann, gamma, d, k0_i, x_j, y, conv, n)
}

/// Цикл Ньютона newton_y v1 (L375-L421; CurveCryptoMath3.vy L202-L248 — тот же текст): K0 = K0_i·y·N/D, S = S_i + y, …,
/// выход при diff < max(conv, y/10^14) с assert frac y. Принимает: A, gamma, D, K0_i, S_i, начальный y, предел
/// сходимости, N. Возвращает: y или None (откат).
#[allow(clippy::too_many_arguments)]
pub fn y_loop(
    ann: U256,
    gamma: U256,
    d: U256,
    k0_i: U256,
    s_i: U256,
    mut y: U256,
    conv: U256,
    n: U256,
) -> Option<U256> {
    let one = U256::from(1u8);
    let two = U256::from(2u8);
    for _ in 0..255 {
        let y_prev = y;
        let k0 = du(mu(mu(k0_i, y)?, n)?, d)?;
        let s = au(s_i, y)?;
        let g = au(gamma, E18)?;
        let g1k0 = if g > k0 { au(su(g, k0)?, one)? } else { au(su(k0, g)?, one)? };
        let m1 = mul1(d, gamma, g1k0, ann)?;
        // mul2 = 10**18 + (2 * 10**18) * K0 / _g1k0.
        let mul2 = au(E18, du(mu(two * E18, k0)?, g1k0)?)?;
        let mut yfprime = au(au(mu(E18, y)?, mu(s, mul2)?)?, m1)?;
        let dyfprime = mu(d, mul2)?;
        if yfprime < dyfprime {
            y = y_prev / two;
            continue;
        }
        yfprime -= dyfprime;
        let fprime = du(yfprime, y)?;
        let mut y_minus = du(m1, fprime)?;
        let y_plus = au(du(au(yfprime, mu(E18, d)?)?, fprime)?, du(mu(y_minus, E18)?, k0)?)?;
        y_minus = au(y_minus, du(mu(E18, s)?, fprime)?)?;
        y = if y_plus < y_minus { y_prev / two } else { y_plus - y_minus };
        let diff = if y > y_prev { y - y_prev } else { y_prev - y };
        if diff < conv.max(y / ten::pow(14)) {
            return frac_ok(y, d)?.then_some(y);
        }
    }
    None
}

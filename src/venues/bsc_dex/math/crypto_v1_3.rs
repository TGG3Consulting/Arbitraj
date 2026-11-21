//! Файл: venues/bsc_dex/math/crypto_v1_3.rs
//! Что делает: математика трёхмонетного crypto-пула Curve первого поколения — tricrypto 2021 (решение 453, контракт
//! математики CurveCryptoMath3.vy Vyper 0.2.15): `sort`, `_geometric_mean`, `newton_D`, `newton_y`. Все операции uint256
//! проверяемые (Vyper 0.2.x): переполнение, деление на ноль, assert, «Did not converge» — None. Цикл Ньютона newton_y —
//! общий с двухмонетным v1 (crypto_v1_2.rs y_loop, тот же текст).
//! Кто вызывает: math/crypto_swap.rs.
//! От чего зависит: math/crypto_ops.rs, math/crypto_v1_2.rs (y_loop, frac_ok), math/crypto_y2.rs (mul1), math/ten.rs.
//! Настройки: нет (числа — константы исходника).
//!
//! Источник: curvefi/curve-crypto-contract a0f8401 (журнал развёртывания «2021-09-13. Arbitrum»: пул
//! 0x960ea3e3C7FB317332d990873d354E18d7645590, математика 0x2F0AF8eC2f5893392843a0F647A30A141dba9DaF; файл математики с
//! 8739cb9 до d7d04cd меняется только строкой версии), contracts/tricrypto/CurveCryptoMath3.vy: константы L8-L15, `sort`
//! L20-L37, `_geometric_mean` L42-L63, `newton_D` L96-L167, `newton_y` L172-L250.

use alloy_primitives::U256;

use super::crypto_ops::{au, du, mu, su};
use super::crypto_v1_2::{frac_ok, y_loop};
use super::crypto_y2::mul1;
use super::ten::{self, E18};

/// N_COINS (L8).
const N: u64 = 3;
/// A_MULTIPLIER (L9).
const A_MULTIPLIER: u64 = 10_000;

/// assert'ы A и gamma (L105-L106, L178-L179): ANN ∈ [27·10000/100; 27·10000·1000] (L14-L15), gamma ∈ [10^10; 5·10^16]
/// (L11-L12). Ошибок нет (false — откат).
fn a_gamma_ok(ann: U256, gamma: U256) -> bool {
    let nn = N * N * N;
    let a =
        (U256::from(nn * A_MULTIPLIER / 100)..=U256::from(nn * A_MULTIPLIER * 1000)).contains(&ann);
    a && (ten::E10..=U256::from(5u8) * ten::pow(16)).contains(&gamma)
}

/// `sort` (L20-L37): сортировка вставками по убыванию — тот же порядок шагов, что в исходнике. Ошибок нет.
fn sort(a0: [U256; 3]) -> [U256; 3] {
    let mut a = a0;
    for i in 1..3 {
        let x = a[i];
        let mut cur = i;
        for _ in 0..3 {
            let y = a[cur - 1];
            if y > x {
                break;
            }
            a[cur] = y;
            cur -= 1;
            if cur == 0 {
                break;
            }
        }
        a[cur] = x;
    }
    a
}

/// `_geometric_mean(x, False)` (L42-L63): D = x0; tmp = Π(tmp·x/D); D = D·((N−1)·10^18 + tmp)/(N·10^18) до diff ≤ 1
/// или diff·10^18 < D. Принимает: x по убыванию. Возвращает: D или None (откат).
fn geometric_mean(x: [U256; 3]) -> Option<U256> {
    let n = U256::from(N);
    let mut d = x[0];
    for _ in 0..255 {
        let d_prev = d;
        let mut tmp = E18;
        for xk in x {
            tmp = du(mu(tmp, xk)?, d)?;
        }
        d = du(mu(d, au((n - U256::from(1u8)) * E18, tmp)?)?, n * E18)?;
        let diff = if d > d_prev { d - d_prev } else { d_prev - d };
        if diff <= U256::from(1u8) || mu(diff, E18)? < d {
            return Some(d);
        }
    }
    None
}

/// `geometric_mean(x)` внешний (L66-L69, sort = True по умолчанию; решение 472 — tweak_price tricrypto 2021): сначала
/// `sort`, затем тот же цикл. Принимает: x в любом порядке. Возвращает: D или None (откат).
pub fn geometric_mean_sorted(x: [U256; 3]) -> Option<U256> {
    geometric_mean(sort(x))
}

/// `newton_D(ANN, gamma, x_unsorted)` (L96-L167). Принимает: A, gamma, балансы в 18 знаках и ценах пула. Возвращает:
/// D или None (откат).
pub fn newton_d(ann: U256, gamma: U256, xu: [U256; 3]) -> Option<U256> {
    if !a_gamma_ok(ann, gamma) {
        return None;
    }
    let x = sort(xu);
    // assert x[0] ∈ [10^9; 10^33] (L111); x[i]·10^18/x[0] > 10^11 − 1 (L112-L114).
    if !(ten::pow(9)..=ten::pow(33)).contains(&x[0]) {
        return None;
    }
    for xk in &x[1..] {
        if du(mu(*xk, E18)?, x[0])? < ten::pow(11) {
            return None;
        }
    }
    let n = U256::from(N);
    let mut d = mu(n, geometric_mean(x)?)?;
    let s = au(au(x[0], x[1])?, x[2])?;
    let one = U256::from(1u8);
    for _ in 0..255 {
        let d_prev = d;
        // K0 = 10**18; for _x in x: K0 = K0 * _x * N_COINS / D (L124-L126).
        let mut k0 = E18;
        for xk in x {
            k0 = du(mu(mu(k0, xk)?, n)?, d)?;
        }
        let g = au(gamma, E18)?;
        let g1k0 = if g > k0 {
            au(su(g, k0)?, one)?
        } else {
            au(su(k0, g)?, one)?
        };
        let m1 = mul1(d, gamma, g1k0, ann)?;
        // mul2 = (2 * 10**18) * N_COINS * K0 / _g1k0; neg_fprime — как у двух монет.
        let mul2 = du(mu(U256::from(2u8) * E18 * n, k0)?, g1k0)?;
        let a = au(au(s, du(mu(s, mul2)?, E18)?)?, du(mu(m1, n)?, k0)?)?;
        let neg_fprime = su(a, du(mu(mul2, d)?, E18)?)?;
        let d_plus = du(mu(d, au(neg_fprime, s)?)?, neg_fprime)?;
        let mut d_minus = du(mu(d, d)?, neg_fprime)?;
        // L145-L148.
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

/// `newton_y(ANN, gamma, x, D, i)` (L172-L250). Принимает: A, gamma, балансы в 18 знаках и ценах пула, D, индекс
/// искомой монеты. Возвращает: y или None (откат).
pub fn newton_y(ann: U256, gamma: U256, x: [U256; 3], d: U256, i: usize) -> Option<U256> {
    if i > 2 || !a_gamma_ok(ann, gamma) || !(ten::E17..=ten::pow(33)).contains(&d) {
        return None;
    }
    // frac других монет в пределах (L182-L184).
    for (k, xk) in x.iter().enumerate() {
        if k != i && !frac_ok(*xk, d)? {
            return None;
        }
    }
    let n = U256::from(N);
    let mut y = du(d, n)?;
    let mut k0_i = E18;
    let mut s_i = U256::ZERO;
    let mut xs = x;
    xs[i] = U256::ZERO;
    let xs = sort(xs);
    let e14 = ten::pow(14);
    let conv = (xs[0] / e14).max(d / e14).max(U256::from(100u8));
    // for j in range(2, N_COINS+1): _x = x_sorted[N_COINS-j]; y = y * D / (_x * N_COINS); S_i += _x (L195-L198).
    for j in 2..=3usize {
        let xv = xs[3 - j];
        y = du(mu(y, d)?, mu(xv, n)?)?;
        s_i = au(s_i, xv)?;
    }
    // for j in range(N_COINS-1): K0_i = K0_i * x_sorted[j] * N_COINS / D (L199-L200).
    for xv in &xs[..2] {
        k0_i = du(mu(mu(k0_i, *xv)?, n)?, d)?;
    }
    y_loop(ann, gamma, d, k0_i, s_i, y, conv, n)
}

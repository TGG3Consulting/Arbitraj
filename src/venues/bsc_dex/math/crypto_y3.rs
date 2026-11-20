//! Файл: venues/bsc_dex/math/crypto_y3.rs
//! Что делает: `get_y` математики Tricrypto-NG (решение 442) — баланс монеты i (в 18 знаках и ценах пула) при балансах
//! двух других монет и инварианте D: аналитическое решение кубического уравнения с масштабированием коэффициентов, при
//! неположительном дискриминанте — запасной метод Ньютона (math/crypto_newton3.rs). Возвращает [y, K0_prev] как
//! контракт. Арифметика — как Vyper (math/crypto_ops.rs): где контракт откатился бы — None.
//! Кто вызывает: math/crypto_swap.rs.
//! От чего зависит: math/crypto_ops.rs, math/crypto_newton3.rs, math/ten.rs.
//! Настройки: нет.
//!
//! Источник: curvefi/tricrypto-ng ecaa816, contracts/main/CurveCryptoMathOptimized3.vy: константы L18-L27, `get_y`
//! L35-L231. Математика, развёрнутая в Arbitrum One (deployments.yaml `arbitrum:mainnet math`
//! 0x604388Bb1159AFd21eB5191cE22b4DeCdEE2Ae22, коммит 2d03e6f «feat: arbitrum deployment»), отличается от ecaa816 только
//! строками pragma (git diff 2d03e6f ecaa816).

use alloy_primitives::{I256, U256};

use super::crypto_newton3::{self, frac_ok};
use super::crypto_ops::{
    abs, add, cbrt, div, i, ie, isqrt, iu, mu, mul, sq, sub, to_i, to_u, uadd, udiv, udiv_u,
    umul, usub,
};
use super::ten::{self, E18, E36};

/// A_MULTIPLIER (L19).
const A_MULTIPLIER: u64 = 10_000;

/// Пределы assert'ов get_y (L49-L51): A ∈ [27·10000/100; 27·10000·1000], gamma ∈ [10^10; 5·10^16],
/// D ∈ [10^17; 10^33]. Ошибок нет (false — откат).
fn safe(ann: U256, gamma: U256, d: U256) -> bool {
    let min_a = U256::from(27 * A_MULTIPLIER / 100);
    let max_a = U256::from(27 * A_MULTIPLIER * 1000);
    let max_g = U256::from(5u8) * ten::pow(16);
    let a_ok = (min_a..=max_a).contains(&ann);
    let g_ok = (ten::E10..=max_g).contains(&gamma);
    a_ok && g_ok && (ten::E17..=ten::pow(33)).contains(&d)
}

/// `get_y(ANN, gamma, x, D, i)` (L35-L231). Принимает: A (как отдаёт пул), gamma, балансы в 18 знаках и ценах пула,
/// D, индекс искомой монеты (0..2). Возвращает: [y, K0_prev] или None (откат контракта).
pub fn get_y(ann_u: U256, gamma_u: U256, x: [U256; 3], d_u: U256, idx: usize) -> Option<[U256; 2]> {
    if idx > 2 || !safe(ann_u, gamma_u, d_u) {
        return None;
    }
    // frac других монет в пределах (L53-L57).
    for (k, xk) in x.iter().enumerate() {
        if k != idx && !frac_ok(du_frac(*xk, d_u)?) {
            return None;
        }
    }
    let (j, k) = match idx {
        0 => (1, 2),
        1 => (0, 2),
        _ => (0, 1),
    };
    let ann = to_i(ann_u)?;
    let gamma = to_i(gamma_u)?;
    let d = to_i(d_u)?;
    let x_j = to_i(x[j])?;
    let x_k = to_i(x[k])?;
    let gamma2 = umul(gamma, gamma);
    let mut a = i(E36 / U256::from(27u8));
    let mut b = b_coef(d, x_j, x_k, gamma, gamma2, ann)?;
    let mut c = c_coef(d, x_j, x_k, gamma, gamma2, ann)?;
    // d = unsafe_div(unsafe_add(10**18, gamma)**2, 27) (L118).
    let mut dd = udiv(sq(uadd(ie(18), gamma))?, iu(27));
    // d0 = abs(unsafe_mul(3, a) * c / b - b) (L121).
    let d0 = abs(sub(div(mul(umul(iu(3), a), c)?, b)?, b)?)?;
    let divider = divider(d0);
    // Масштаб коэффициентов (L141-L152).
    if abs(a)? > abs(b)? {
        let prec = abs(udiv(a, b))?;
        a = udiv(umul(a, prec), divider);
        b = udiv(mul(b, prec)?, divider);
        c = udiv(mul(c, prec)?, divider);
        dd = udiv(mul(dd, prec)?, divider);
    } else {
        let prec = abs(udiv(b, a))?;
        a = udiv(div(a, prec)?, divider);
        b = udiv(udiv(b, prec), divider);
        c = udiv(udiv(c, prec), divider);
        dd = udiv(udiv(dd, prec), divider);
    }
    // delta0 = unsafe_div(_3ac, b) - b; delta1 (L155-L164).
    let ac3 = mul(umul(iu(3), a), c)?;
    let delta0 = sub(udiv(ac3, b), b)?;
    let t = udiv(mul(udiv(mul(iu(27), sq(a)?)?, b), dd)?, b);
    let delta1 = sub(sub(udiv(mul(iu(3), ac3)?, b), umul(iu(2), b))?, t)?;
    // sqrt_arg = delta1**2 + unsafe_div(4 * delta0**2, b) * delta0 (L167-L170).
    let tail = mul(udiv(mul(iu(4), sq(delta0)?)?, b), delta0)?;
    let sqrt_arg = add(sq(delta1)?, tail)?;
    if sqrt_arg <= I256::ZERO {
        let y = crypto_newton3::newton_y(ann_u, gamma_u, x, d_u, idx)?;
        return Some([y, U256::ZERO]);
    }
    let sqrt_val = to_i(isqrt(to_u(sqrt_arg)?))?;
    let root_k0 = root_k0(b, delta0, delta1, sqrt_val)?;
    // root = D*D/27/x_k*D/x_j*root_K0/a (L210-L217): unsafe, кроме D*D, *D и *root_K0.
    let r = udiv(mul(udiv(udiv(mul(d, d)?, iu(27)), x_k), d)?, x_j);
    let root = udiv(mul(r, root_k0)?, a);
    let y = to_u(root)?;
    let k0 = to_u(udiv(mul(ie(18), root_k0)?, a))?;
    // frac = unsafe_div(out[0] * 10**18, D) — assert frac >= 10**16 - 1 and frac < 10**20 + 1 (L224-L225).
    let frac = udiv_u(mu(y, E18)?, d_u);
    let lo = ten::pow(16) - U256::from(1u8);
    if !(lo..=ten::pow(20)).contains(&frac) {
        return None;
    }
    Some([y, k0])
}

/// Проверяемое x * 10**18 / D (frac из L55). None — откат.
fn du_frac(x: U256, d: U256) -> Option<U256> {
    mu(x, E18)?.checked_div(d)
}

/// b (L82-L101): 10**36/9 + 2·10**18·gamma/27 − D²/x_j·gamma²·ANN/27²/A_MULTIPLIER/x_k. None — откат.
fn b_coef(d: I256, x_j: I256, x_k: I256, gamma: I256, gamma2: I256, ann: I256) -> Option<I256> {
    let ninth = i(E36 / U256::from(9u8));
    let first = uadd(ninth, udiv(umul(ie(18) * iu(2), gamma), iu(27)));
    let v = mul(umul(udiv(umul(d, d), x_j), gamma2), ann)?;
    let second = udiv(udiv(udiv(v, iu(27 * 27)), iu(A_MULTIPLIER)), x_k);
    sub(first, second)
}

/// c (L104-L115): 10**36/9 + gamma·(gamma + 4·10**18)/27 + gamma²·(x_j + x_k − D)/D·ANN/27/A_MULTIPLIER.
/// None — откат.
fn c_coef(d: I256, x_j: I256, x_k: I256, gamma: I256, gamma2: I256, ann: I256) -> Option<I256> {
    let g4 = uadd(gamma, ie(18) * iu(4));
    let ninth = i(E36 / U256::from(9u8));
    let first = uadd(ninth, udiv(umul(gamma, g4), iu(27)));
    let v = udiv(mul(gamma2, usub(uadd(x_j, x_k), d))?, d);
    let second = udiv(udiv(umul(v, ann), iu(27)), iu(A_MULTIPLIER));
    add(first, second)
}

/// Делитель по d0 (L123-L139): цепочка elif исходника. Ошибок нет.
fn divider(d0: I256) -> I256 {
    // Пороги 10^p и делители 10^q по убыванию p.
    const P: [u64; 8] = [48, 44, 40, 36, 32, 28, 24, 20];
    const Q: [u64; 8] = [30, 26, 22, 18, 14, 10, 6, 2];
    for (p, q) in P.into_iter().zip(Q) {
        if d0 > ie(p) {
            return ie(q);
        }
    }
    iu(1)
}

/// root_K0 (L179-L207): кубические корни b и (delta1 ± sqrt_val)/2, C1, (b + b·delta0/C1 − C1)/3. None — откат.
fn root_k0(b: I256, delta0: I256, delta1: I256, sqrt_val: I256) -> Option<I256> {
    let two = U256::from(2u8);
    let b_cbrt = if b >= I256::ZERO {
        to_i(cbrt(to_u(b)?))?
    } else {
        neg(to_i(cbrt(to_u(neg(b)?)?))?)?
    };
    let second_cbrt = if delta1 > I256::ZERO {
        to_i(cbrt(udiv_u(to_u(add(delta1, sqrt_val)?)?, two)))?
    } else {
        let v = to_u(neg(sub(delta1, sqrt_val)?)?)?;
        neg(to_i(cbrt(udiv_u(v, two)))?)?
    };
    // C1 = unsafe_div(unsafe_div(b_cbrt * b_cbrt, 10**18) * second_cbrt, 10**18) (L199-L202).
    let c1 = udiv(mul(udiv(mul(b_cbrt, b_cbrt)?, ie(18)), second_cbrt)?, ie(18));
    // root_K0 = unsafe_div(b + b * delta0 / C1 - C1, 3) (L205).
    let s = sub(add(b, div(mul(b, delta0)?, c1)?)?, c1)?;
    Some(udiv(s, iu(3)))
}

/// Унарный минус int256 (проверяемый). None — переполнение.
fn neg(v: I256) -> Option<I256> {
    I256::ZERO.checked_sub(v)
}

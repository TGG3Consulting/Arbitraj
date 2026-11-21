//! Файл: venues/bsc_dex/math/crypto_p3.rs
//! Что делает: решение 472 — `get_p` математики Tricrypto-NG (цены dx/dy и dx/dz при балансах xp и инварианте D,
//! 18 знаков, без price_scale). Нужна модели `tweak_price` после обмена (math/crypto_tweak3.rs). Арифметика Vyper
//! 0.3.10 (math/crypto_ops.rs): проверяемые операции — откат → None, unsafe_* и pow_mod256 — по модулю 2^256.
//! Кто вызывает: math/crypto_tweak3.rs.
//! От чего зависит: math/crypto_ops.rs, math/ten.rs.
//! Настройки: нет.
//!
//! Источник: curvefi/tricrypto-ng ecaa816, contracts/main/CurveCryptoMathOptimized3.vy `get_p` L539-L593.

use alloy_primitives::U256;

use super::crypto_ops::{au, du, mu, su, udiv_u};
use super::ten::{self, E18, E36};

/// A_MULTIPLIER (L19).
const A_MULTIPLIER: u64 = 10_000;

/// `get_p(_xp, _D, [A, gamma])`. Принимает: балансы в 18 знаках и ценах пула, D, A (как отдаёт пул), gamma.
/// Возвращает: [p_xy, p_xz] в 18 знаках или None (откат: D вне [10^17; 10^33] — L550, переполнение проверяемой
/// операции, вычитание ниже нуля, деление на ноль `/ _xp[1]`, `/ _xp[2]`).
pub fn get_p(xp: [U256; 3], d: U256, a: U256, gamma: U256) -> Option<[U256; 2]> {
    if d < ten::pow(17) || d > ten::pow(33) {
        return None;
    }
    // K0 = unsafe_div(unsafe_div(unsafe_div(27 * _xp[0] * _xp[1], _D) * _xp[2], _D) * 10**36, _D) (L554-L557).
    let k = udiv_u(mu(mu(U256::from(27u8), xp[0])?, xp[1])?, d);
    let k0 = udiv_u(mu(udiv_u(mu(k, xp[2])?, d), E36)?, d);
    // GK0 (L566-L573) — как у Twocrypto (math/crypto_p2.rs).
    let t1 = udiv_u(mu(udiv_u(mu(mu(U256::from(2u8), k0)?, k0)?, E36), k0)?, E36);
    let g1 = gamma.wrapping_add(E18);
    let t2 = g1.wrapping_mul(g1);
    let g3 = U256::from(2u8)
        .wrapping_mul(gamma)
        .wrapping_add(U256::from(3u8) * E18);
    let t3 = udiv_u(mu(udiv_u(k0.wrapping_mul(k0), E36), g3)?, E18);
    let gk0 = su(au(t1, t2)?, t3)?;
    // NNAG2 = unsafe_div(unsafe_mul(A, pow_mod256(gamma, 2)), A_MULTIPLIER) (L576).
    let nnag2 = udiv_u(
        a.wrapping_mul(gamma.wrapping_mul(gamma)),
        U256::from(A_MULTIPLIER),
    );
    // denominator = GK0 + unsafe_div(unsafe_div(NNAG2 * _xp[0], _D) * K0, 10**36) (L579).
    let den = au(gk0, udiv_u(mu(udiv_u(mu(nnag2, xp[0])?, d), k0)?, E36))?;
    // p_k = unsafe_div(_xp[0] * (GK0 + unsafe_div(unsafe_div(NNAG2 * _xp[k], _D) * K0, 10**36)) / _xp[k] * 10**18,
    //   denominator) (L584-L593).
    let p = |xk: U256| -> Option<U256> {
        let num = au(gk0, udiv_u(mu(udiv_u(mu(nnag2, xk)?, d), k0)?, E36))?;
        Some(udiv_u(mu(du(mu(xp[0], num)?, xk)?, E18)?, den))
    };
    Some([p(xp[1])?, p(xp[2])?])
}

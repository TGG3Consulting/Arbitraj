//! Файл: venues/bsc_dex/math/crypto_p2.rs
//! Что делает: решение 472 — `get_p` математики Twocrypto-NG (цена dx/dy при балансах xp и инварианте D, 18 знаков,
//! без price_scale). Нужна модели `tweak_price` после обмена (math/crypto_tweak2.rs): пул зовёт её на каждом обмене,
//! её assert и проверяемые операции могут откатить обмен. Арифметика Vyper 0.3.10 (math/crypto_ops.rs): проверяемые
//! операции — откат → None, unsafe_* и pow_mod256 — по модулю 2^256.
//! Кто вызывает: math/crypto_tweak2.rs.
//! От чего зависит: math/crypto_ops.rs, math/ten.rs.
//! Настройки: нет.
//!
//! Источник: curvefi/twocrypto-ng 89c8dfb, contracts/main/CurveCryptoMathOptimized2.vy `get_p` L465-L512; у d612fde
//! (v2.0.0) — тот же текст (diff функций пуст).

use alloy_primitives::U256;

use super::crypto_ops::{au, du, mu, su, udiv_u};
use super::ten::{self, E18, E36};

/// A_MULTIPLIER (L18).
const A_MULTIPLIER: u64 = 10_000;

/// `get_p(_xp, _D, [A, gamma])`. Принимает: балансы в 18 знаках и ценах пула, D, A (как отдаёт пул), gamma.
/// Возвращает: цену в 18 знаках или None (откат: D вне [10^17; 10^33] — L476, переполнение проверяемой операции,
/// вычитание ниже нуля, деление на ноль `/ _xp[1]`).
pub fn get_p(xp: [U256; 2], d: U256, a: U256, gamma: U256) -> Option<U256> {
    // assert _D > 10**17 - 1 and _D < 10**15 * 10**18 + 1 (L476).
    if d < ten::pow(17) || d > ten::pow(33) {
        return None;
    }
    // K0 = unsafe_div(unsafe_div(4 * _xp[0] * _xp[1], _D) * 10**36, _D) (L480-L483).
    let k0 = udiv_u(
        mu(udiv_u(mu(mu(U256::from(4u8), xp[0])?, xp[1])?, d), E36)?,
        d,
    );
    // GK0 = unsafe_div(unsafe_div(2*K0*K0, 10**36) * K0, 10**36) + pow_mod256(gamma + 10**18, 2)
    //   - unsafe_div(unsafe_div(pow_mod256(K0, 2), 10**36) * (2*gamma + 3*10**18), 10**18) (L492-L499).
    let t1 = udiv_u(mu(udiv_u(mu(mu(U256::from(2u8), k0)?, k0)?, E36), k0)?, E36);
    let g1 = gamma.wrapping_add(E18);
    let t2 = g1.wrapping_mul(g1);
    let g3 = U256::from(2u8)
        .wrapping_mul(gamma)
        .wrapping_add(U256::from(3u8) * E18);
    let t3 = udiv_u(mu(udiv_u(k0.wrapping_mul(k0), E36), g3)?, E18);
    let gk0 = su(au(t1, t2)?, t3)?;
    // NNAG2 = unsafe_div(unsafe_mul(A, pow_mod256(gamma, 2)), A_MULTIPLIER) (L502).
    let nnag2 = udiv_u(
        a.wrapping_mul(gamma.wrapping_mul(gamma)),
        U256::from(A_MULTIPLIER),
    );
    // denominator = GK0 + unsafe_div(unsafe_div(NNAG2 * _xp[0], _D) * K0, 10**36) (L505).
    let den = au(gk0, udiv_u(mu(udiv_u(mu(nnag2, xp[0])?, d), k0)?, E36))?;
    // unsafe_div(_xp[0] * (GK0 + unsafe_div(unsafe_div(NNAG2 * _xp[1], _D) * K0, 10**36)) / _xp[1] * 10**18,
    //   denominator) (L509-L512).
    let num = au(gk0, udiv_u(mu(udiv_u(mu(nnag2, xp[1])?, d), k0)?, E36))?;
    let p = mu(du(mu(xp[0], num)?, xp[1])?, E18)?;
    Some(udiv_u(p, den))
}

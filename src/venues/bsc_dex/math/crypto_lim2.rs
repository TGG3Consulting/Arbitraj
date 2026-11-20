//! Файл: venues/bsc_dex/math/crypto_lim2.rs
//! Что делает: пределы assert'ов математики Twocrypto-NG по версии исходника (решение 453) — v2.1.0 и v2.0.0 отличаются
//! только ими: MAX_GAMMA, пределы K0_i (get_y и _newton_y), доли y (get_y) и доли x (newton_D). Формулы get_y и
//! newton_D у версий одинаковы (git diff d612fde 89c8dfb — только эти строки и lim_mul).
//! Кто вызывает: math/crypto_y2.rs, math/crypto_d2.rs.
//! От чего зависит: core/types (CryptoMath), math/crypto_ops.rs, math/ten.rs.
//! Настройки: нет (числа — константы исходника контракта).
//!
//! Источник: curvefi/twocrypto-ng, contracts/main/CurveCryptoMathOptimized2.vy:
//! - v2.1.0 — 89c8dfb: MAX_GAMMA_SMALL / MAX_GAMMA L21-L22, MIN_A / MAX_A L24-L25, lim_mul L241-L243, K0_i L257, frac
//!   y L369, frac x в newton_D L457, assert'ы A / gamma / D L238-L240, L384-L385;
//! - v2.0.0 — d612fde: MAX_GAMMA = 2·10^15 L21, MIN_A / MAX_A L23-L24, K0_i L249 (и _newton_y L153), frac y L361,
//!   frac x в newton_D L449, assert'ы L234-L236, L376-L377.
//!
//! Что v2.0.0 развёрнута в Arbitrum One именно из d612fde — факт: официальный скрипт развёртывания этого коммита
//! (scripts/deploy_infra.py: CREATE2 через 0x13b0D85CcB8bf860b6b79AF3029fCA081AE9beF2, соль keccak(42069), assert
//! precomputed_address == calculated_address) и наша перекомпиляция исходника d612fde Vyper 0.3.10 дают ровно адреса
//! deployments.yaml `math_v2_0_0` 0x2005995a71243be9FB995DaB4742327dc76564Df и `amm_v2_0_0`
//! 0x04Fd6beC7D45EFA99a27D29FB94b55c56dD07223 (так же 99b5192 → math 0x1Fd8Af16…, amm 0x934791f7… v2.1.0).

use alloy_primitives::U256;

use crate::core::types::CryptoMath;

use super::crypto_ops::udiv_u;
use super::ten::{self, E18, E36};

/// N_COINS (L17).
const N: u64 = 2;
/// A_MULTIPLIER (L18).
const A_MULTIPLIER: u64 = 10_000;

/// Пределы одной версии. Все интервалы — включительно (assert'ы исходника приведены к [lo; hi]).
pub struct Lim {
    /// lim_mul v2.1.0 (L241-L243); у v2.0.0 его нет — 0.
    pub lim: U256,
    /// MAX_GAMMA версии.
    pub max_gamma: U256,
    /// K0_i ∈ [lo; hi] (get_y и _newton_y).
    pub k0: (U256, U256),
    /// frac = y·10^18/D ∈ [lo; hi] (get_y, конец).
    pub frac_y: (U256, U256),
    /// frac = x·10^18/D ∈ [lo; hi] (newton_D, конец).
    pub frac_x: (U256, U256),
}

/// Пределы версии при данном gamma. Принимает: версию (TwoNg210 / TwoNg200; прочие — как v2.1.0, их сюда не зовут),
/// gamma. Ошибок нет.
pub fn of(m: CryptoMath, gamma: U256) -> Lim {
    if m == CryptoMath::TwoNg200 {
        // K0_i > 10**16*N - 1 and K0_i < 10**20*N + 1; frac >= 10**16 - 1 and frac < 10**20 + 1.
        let n = U256::from(N);
        return Lim {
            lim: U256::ZERO,
            max_gamma: U256::from(2u8) * ten::pow(15),
            k0: (ten::pow(16) * n, ten::pow(20) * n),
            frac_y: (ten::pow(16) - U256::from(1u8), ten::pow(20)),
            frac_x: (ten::pow(16) - U256::from(1u8), ten::pow(20)),
        };
    }
    let lim = lim_mul(gamma);
    let two = U256::from(N);
    Lim {
        lim,
        max_gamma: U256::from(199u64) * ten::pow(15),
        // K0_i >= unsafe_div(10**36, lim_mul) and K0_i <= lim_mul (L257).
        k0: (udiv_u(E36, lim), lim),
        // frac >= unsafe_div(10**36 / N_COINS, lim_mul) and frac <= unsafe_div(lim_mul, N_COINS) (L369).
        frac_y: (udiv_u(E36 / two, lim), udiv_u(lim, two)),
        // frac > 10**16 / N_COINS - 1 and frac < 10**20 / N_COINS + 1 (L457).
        frac_x: (ten::pow(16) / two, ten::pow(20) / two),
    }
}

/// lim_mul (L241-L243): 100·10^18, при gamma > MAX_GAMMA_SMALL (2·10^16) — unsafe_div(100·10^18·2·10^16, gamma).
/// Ошибок нет.
fn lim_mul(gamma: U256) -> U256 {
    let base = U256::from(100u8) * E18;
    let small = U256::from(2u8) * ten::pow(16);
    if gamma > small {
        udiv_u(base.wrapping_mul(small), gamma)
    } else {
        base
    }
}

/// assert'ы A и gamma (get_y, newton_D): ANN ∈ [MIN_A; MAX_A] (MIN_A = N^N·A_MULTIPLIER/10,
/// MAX_A = N^N·A_MULTIPLIER·1000), gamma ∈ [MIN_GAMMA = 10^10; MAX_GAMMA версии]. Ошибок нет (false — откат).
pub fn a_gamma_ok(l: &Lim, ann: U256, gamma: U256) -> bool {
    let nn = N * N;
    let min_a = U256::from(nn * A_MULTIPLIER / 10);
    let max_a = U256::from(nn * A_MULTIPLIER * 1000);
    (min_a..=max_a).contains(&ann) && (ten::E10..=l.max_gamma).contains(&gamma)
}

/// assert D get_y: D ∈ [10^17; 10^33]. Ошибок нет.
pub fn d_ok(d: U256) -> bool {
    (ten::E17..=ten::pow(33)).contains(&d)
}

/// Принадлежность включительному интервалу. Ошибок нет.
pub fn within(v: U256, r: (U256, U256)) -> bool {
    (r.0..=r.1).contains(&v)
}

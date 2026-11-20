//! Файл: venues/bsc_dex/math/balancer_v3_weighted.rs
//! Что делает: решение 441 (T433-F2) — FixedPoint V3 (powUp с частными случаями, complement, mulDivUp) и формулы
//! WeightedMath Balancer V3 (выход на точный вход, вход на точный выход). Вынесено из math/balancer_v3.rs (предел
//! размера файла).
//! Кто вызывает: math/balancer_v3.rs.
//! От чего зависит: math/balancer_math.rs (mulDown, mulUp, divDown, divUp, ONE — те же у V2 и V3: FixedPoint.sol
//! L20-L48), math/balancer_log_exp.rs (LogExpMath.pow: у V3 тот же алгоритм — сверено по тексту, решение 441).
//! Настройки: нет.
//!
//! Источник (решение 23): balancer/balancer-deployments@b55dc2b8, build-info v3/tasks/20260115-v3-weighted-pool-v2 →
//! @balancer-labs/v3-solidity-utils/contracts/math/FixedPoint.sol L51-L71 (mulDivUp), L124-L140 (powUp), L148-L154
//! (complement); WeightedMath.sol L54-L55 (MAX_IN_RATIO / MAX_OUT_RATIO = 30 %), computeOutGivenExactIn L173-L208,
//! computeInGivenExactOut L219-L255 (у 20241205-v3-weighted-pool — те же формулы, L171-L205 / L216-L255).

use alloy_primitives::U256;

use super::balancer_log_exp;
use super::balancer_math::{div_down, div_up, mul_down, mul_up, one};

/// mulDivUp (FixedPoint.sol L51-L71): c = 0 — None; произведение 0 → 0, иначе (a·b − 1) / c + 1. None — переполнение.
pub fn mul_div_up(a: U256, b: U256, c: U256) -> Option<U256> {
    if c.is_zero() {
        return None;
    }
    let p = a.checked_mul(b)?;
    Some(if p.is_zero() {
        p
    } else {
        (p - U256::from(1u8)) / c + U256::from(1u8)
    })
}

/// powUp V3 (FixedPoint.sol L124-L140). None — откат pow или переполнение.
fn pow_up(x: U256, y: U256) -> Option<U256> {
    let one = one();
    if y == one {
        return Some(x);
    }
    if y == one * U256::from(2u8) {
        return mul_up(x, x);
    }
    if y == one * U256::from(4u8) {
        let sq = mul_up(x, x)?;
        return mul_up(sq, sq);
    }
    let raw = balancer_log_exp::pow(x, y)?;
    raw.checked_add(mul_up(raw, U256::from(10_000u32))?)?
        .checked_add(U256::from(1u8))
}

/// complement (FixedPoint.sol L148-L154): 1 − x, не меньше нуля.
pub fn complement(x: U256) -> U256 {
    if x < one() { one() - x } else { U256::ZERO }
}

/// 30 % — _MAX_IN_RATIO / _MAX_OUT_RATIO (WeightedMath.sol L54-L55).
fn max_ratio() -> U256 {
    U256::from(3u8) * super::ten::E17
}

/// computeOutGivenExactIn (WeightedMath.sol L173-L208). None — вход > 30 % баланса или откат арифметики.
pub fn weighted_out(b_in: U256, w_in: U256, b_out: U256, w_out: U256, a_in: U256) -> Option<U256> {
    if a_in > mul_down(b_in, max_ratio())? {
        return None;
    }
    let base = div_up(b_in, b_in.checked_add(a_in)?)?;
    let power = pow_up(base, div_down(w_in, w_out)?)?;
    mul_down(b_out, complement(power))
}

/// computeInGivenExactOut (WeightedMath.sol L219-L255). None — выход > 30 % баланса или откат арифметики.
pub fn weighted_in(b_in: U256, w_in: U256, b_out: U256, w_out: U256, a_out: U256) -> Option<U256> {
    if a_out > mul_down(b_out, max_ratio())? {
        return None;
    }
    let base = div_up(b_out, b_out.checked_sub(a_out)?)?;
    let power = pow_up(base, div_up(w_out, w_in)?)?;
    mul_up(b_in, power.checked_sub(one())?)
}

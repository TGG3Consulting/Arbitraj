//! Файл: venues/bsc_dex/math/swap_step.rs
//! Что делает: один шаг обмена внутри диапазона тика (SwapMath.computeSwapStep) — две версии:
//! V3 (PancakeSwap/Uniswap V3: amountRemaining ≥ 0 — точный вход) и V4 (Infinity CL, Uniswap V4:
//! amountRemaining < 0 — точный вход, другая формула комиссии при недостижении цели).
//! Кто вызывает: math/swap_cl.rs.
//! От чего зависит: math/sqrt_price_math.rs, math/full_math.rs.
//! Настройки: нет.
//!
//! Источники: V3 — https://github.com/pancakeswap/pancake-v3-contracts/blob/986847948755cba528324d41be19480731c36c2a/projects/v3-core/contracts/libraries/SwapMath.sol#L21-L97 ;
//! V4 — https://github.com/Uniswap/v4-core/blob/bea7f6d1c11cd5dc4f0d5151ffa37a1c239feee6/src/libraries/SwapMath.sol#L51-L107
//! (Infinity CL — то же: infinity-core src/pool-cl/libraries/SwapMath.sol#L51-L107, research/flow/bsc_v4.md, раздел 5).
//! Комиссия — единицы 1e-6 (MAX_SWAP_FEE = 1e6).

use alloy_primitives::{I256, U256};

use super::full_math::{mul_div, mul_div_up};
use super::sqrt_price_math::{amount0_delta, amount1_delta, next_from_input, next_from_output};

/// 1e6 — знаменатель комиссии в пипсах.
const MAX_FEE: u32 = 1_000_000;

/// Итог шага: (новая цена, вход без комиссии, выход, комиссия).
pub type Step = (U256, U256, U256, U256);

/// Сумма входа до цели (вход округляется вверх).
fn in_to(cur: U256, target: U256, l: u128, z: bool) -> Option<U256> {
    if z {
        amount0_delta(target, cur, l, true)
    } else {
        amount1_delta(cur, target, l, true)
    }
}

/// Сумма выхода до цели (выход округляется вниз).
fn out_to(cur: U256, next: U256, l: u128, z: bool) -> Option<U256> {
    if z {
        amount1_delta(next, cur, l, false)
    } else {
        amount0_delta(cur, next, l, false)
    }
}

/// Шаг V3. amount ≥ 0 — точный вход, < 0 — точный выход. None — там, где контракт откатился бы.
pub fn step_v3(cur: U256, target: U256, l: u128, amount: I256, fee: u32) -> Option<Step> {
    let z = cur >= target;
    let exact_in = !amount.is_negative();
    let rem = amount.unsigned_abs();
    let f = U256::from(fee);
    let m = U256::from(MAX_FEE);
    let (mut a_in, mut a_out) = (U256::ZERO, U256::ZERO);
    let next = if exact_in {
        let less_fee = mul_div(rem, m - f, m)?;
        a_in = in_to(cur, target, l, z)?;
        if less_fee >= a_in {
            target
        } else {
            next_from_input(cur, l, less_fee, z)?
        }
    } else {
        a_out = out_to(cur, target, l, z)?;
        if rem >= a_out {
            target
        } else {
            next_from_output(cur, l, rem, z)?
        }
    };
    let max = target == next;
    if !(max && exact_in) {
        a_in = if z {
            amount0_delta(next, cur, l, true)?
        } else {
            amount1_delta(cur, next, l, true)?
        };
    }
    if !(max && !exact_in) {
        a_out = out_to(cur, next, l, z)?;
    }
    if !exact_in && a_out > rem {
        a_out = rem;
    }
    let fee_amount = if exact_in && next != target {
        rem - a_in
    } else {
        mul_div_up(a_in, f, m - f)?
    };
    Some((next, a_in, a_out, fee_amount))
}

/// Шаг V4 (Infinity CL, Uniswap V4). amount < 0 — точный вход, > 0 — точный выход. None — откат контракта.
pub fn step_v4(cur: U256, target: U256, l: u128, amount: I256, fee: u32) -> Option<Step> {
    let z = cur >= target;
    let exact_in = amount.is_negative();
    let rem = amount.unsigned_abs();
    let f = U256::from(fee);
    let m = U256::from(MAX_FEE);
    if exact_in {
        let less_fee = mul_div(rem, m - f, m)?;
        let mut a_in = in_to(cur, target, l, z)?;
        let (next, fee_amount) = if less_fee >= a_in {
            let fa = if fee == MAX_FEE {
                a_in
            } else {
                mul_div_up(a_in, f, m - f)?
            };
            (target, fa)
        } else {
            a_in = less_fee;
            (next_from_input(cur, l, less_fee, z)?, rem - a_in)
        };
        Some((next, a_in, out_to(cur, next, l, z)?, fee_amount))
    } else {
        if fee >= MAX_FEE {
            return None;
        }
        let mut a_out = out_to(cur, target, l, z)?;
        let next = if rem >= a_out {
            target
        } else {
            a_out = rem;
            next_from_output(cur, l, a_out, z)?
        };
        let a_in = if z {
            amount0_delta(next, cur, l, true)?
        } else {
            amount1_delta(cur, next, l, true)?
        };
        Some((next, a_in, a_out, mul_div_up(a_in, f, m - f)?))
    }
}

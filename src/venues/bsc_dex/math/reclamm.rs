//! Файл: venues/bsc_dex/math/reclamm.rs
//! Что делает: решение 452 — onSwap пула ReClamm (Balancer V3): постоянное произведение по (реальный + виртуальный
//! баланс) двух токенов A (индекс 0) и B (индекс 1); выход не больше реального баланса. Виртуальные балансы — на блоке
//! чтения (pools/load_balancer_v3_reclamm.rs: computeCurrentVirtualBalances пула — тот же _computeCurrentVirtualBalances,
//! что onSwap, при тех же живых балансах).
//! Кто вызывает: math/balancer_v3_pool.rs.
//! От чего зависит: math/balancer_v3_weighted.rs (mulDivUp FixedPoint V3).
//! Настройки: нет.
//!
//! Источник (решение 23): balancer/balancer-deployments@b55dc2b8 build-info v3/tasks/20260428-v3-reclamm-pool-v3 (код —
//! balancer/reclamm@17262629) → contracts/lib/ReClammMath.sol a = 0, b = 1 (L22-L23), computeOutGivenIn L132-L175
//! ((Bo + Vo)·Ai / (Bi + Vi + Ai), целое деление; > Bo — откат), computeInGivenOut L187-L234 (Ao > Bo — откат;
//! mulDivUp(Bi + Vi, Ao, Bo + Vo − Ao)); contracts/ReClammPool.sol onSwap L183-L214. У
//! v3/deprecated/20250409-v3-reclamm-pool и 20250702-v3-reclamm-pool-v2 те же формулы и onSwap (сверено по тексту).

use alloy_primitives::U256;

use super::balancer_v3_weighted::mul_div_up;

/// onSwap. Принимает: живые балансы (2), виртуальные балансы A и B, индексы, сумму (18 знаков), точный ли вход.
/// Возвращает: выход или вход (18 знаков). None — выход больше реального баланса, переполнение, деление на ноль.
pub fn on_swap(
    bal: &[U256],
    va: U256,
    vb: U256,
    i: usize,
    j: usize,
    a: U256,
    exact_in: bool,
) -> Option<U256> {
    if bal.len() != 2 || i > 1 || j > 1 || i == j {
        return None;
    }
    let (v_in, v_out) = if i == 0 { (va, vb) } else { (vb, va) };
    let (b_in, b_out) = (bal[i], bal[j]);
    if exact_in {
        let num = b_out.checked_add(v_out)?.checked_mul(a)?;
        let den = b_in.checked_add(v_in)?.checked_add(a)?;
        let out = num.checked_div(den)?;
        (out <= b_out).then_some(out)
    } else {
        if a > b_out {
            return None;
        }
        let den = b_out.checked_add(v_out)?.checked_sub(a)?;
        mul_div_up(b_in.checked_add(v_in)?, a, den)
    }
}

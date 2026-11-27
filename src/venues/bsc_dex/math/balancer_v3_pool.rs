//! Файл: venues/bsc_dex/math/balancer_v3_pool.rs
//! Что делает: onSwap пула Balancer V3 по его правилу (выпуск фабрики): решение 441 — весовой (20241205; 20260115 — вход + 1
//! и нижние балансы) и стейблсвап (без проверки / с проверкой дисбаланса); решение 452 — QuantAMM (весовой с пределом
//! доли сделки), LBP (math/balancer_v3_lbp.rs), Gyro 2-CLP (math/gyro_2clp.rs), Gyro E-CLP (math/gyro_eclp.rs), ReClamm
//! (math/reclamm.rs). Суммы и балансы — живые 18 знаков; комиссию снимает хранилище снаружи (math/balancer_v3.rs).
//! Вынесено из math/balancer_v3.rs (предел размера файла).
//! Кто вызывает: math/balancer_v3.rs (обмен), math/balancer_v3_hook.rs (StableSurge зовёт onSwap пула сам).
//! От чего зависит: core/types (VaultCurve, VaultV3, VaultV3Rule), math/balancer_v3_weighted.rs, balancer_v3_stable.rs,
//! balancer_v3_lbp.rs, gyro_2clp.rs, gyro_eclp.rs, reclamm.rs, balancer_math.rs (mulDown).
//! Настройки: нет.
//!
//! Источник (решение 23): balancer/balancer-deployments@b55dc2b8, build-info: весовой — шапка math/balancer_v3.rs
//! (WeightedPool.sol 20241205 onSwap L150-L176, 20260115 L172-L208); стейблсвап — StablePool.sol 20241205 L169-L196,
//! 20260116 L195-L247. QuantAMM — QuantAMMProtocol/deployment@e7867165 v3/tasks/20250429-v3-arbitrum-quantamm
//! build-info QuantAMMWeightedPool.json → contracts/QuantAMMWeightedPool.sol onSwap L271-L340 (maxTradeSizeRatio до и
//! после WeightedMath; WeightedMath, FixedPoint, LogExpMath — те же, что у 20241205, сверено по тексту).

use alloy_primitives::U256;

use crate::core::types::pool_vault::{VaultCurve, VaultV3, VaultV3Rule};

use super::balancer_math::mul_down;
use super::balancer_v3_stable as stable;
use super::balancer_v3_weighted::{weighted_in, weighted_out};
use super::{balancer_v3_lbp, gyro_2clp, gyro_eclp, reclamm};

/// onSwap пула. Принимает: кривую, часть V3, индексы, сумму (18 знаков), точный ли вход. Возвращает: выход (точный вход)
/// или вход без комиссии (точный выход), 18 знаков. None — откат пула или кривая не совпала с правилом.
pub fn on_swap(
    curve: &VaultCurve,
    v3: &VaultV3,
    i: usize,
    j: usize,
    a: U256,
    exact_in: bool,
) -> Option<U256> {
    let bal = &v3.live;
    match (&v3.rule, curve) {
        (VaultV3Rule::PuzzleRanges {virtual_balances, enabled, ..}, VaultCurve::Weighted {weights}) => {
            if !enabled {return None}
            super::puzzle_ranges::on_swap(virtual_balances,bal,weights,i,j,a,exact_in)
        }
        (VaultV3Rule::Weighted, VaultCurve::Weighted { weights }) => {
            weighted(weights, bal, i, j, a, exact_in, None)
        }
        (VaultV3Rule::WeightedMin { min }, VaultCurve::Weighted { weights }) => {
            weighted(weights, bal, i, j, a, exact_in, Some(min.as_slice()))
        }
        (VaultV3Rule::Stable | VaultV3Rule::StableGuard, VaultCurve::Stable { amp }) => {
            let guard = matches!(v3.rule, VaultV3Rule::StableGuard);
            stable_swap(*amp, bal, i, j, a, exact_in, guard)
        }
        (VaultV3Rule::QuantAmm { max_trade }, VaultCurve::Weighted { weights }) => {
            quant_amm(weights, bal, i, j, a, exact_in, *max_trade)
        }
        (VaultV3Rule::Lbp(l), VaultCurve::Weighted { weights }) => {
            balancer_v3_lbp::on_swap(l, weights, bal, i, j, a, exact_in)
        }
        (
            VaultV3Rule::FixedLbp {
                enabled,
                project,
                rate,
            },
            VaultCurve::V3,
        ) => balancer_v3_lbp::fixed(*enabled, *project, *rate, i, a, exact_in),
        (
            VaultV3Rule::Gyro2Clp {
                sqrt_alpha,
                sqrt_beta,
            },
            VaultCurve::V3,
        ) => gyro_2clp::on_swap(bal, i, j, a, exact_in, *sqrt_alpha, *sqrt_beta),
        (VaultV3Rule::GyroEclp(p), VaultCurve::V3) => gyro_eclp::on_swap(p, bal, i, j, a, exact_in),
        (VaultV3Rule::ReClamm { va, vb }, VaultCurve::V3) => {
            reclamm::on_swap(bal, *va, *vb, i, j, a, exact_in)
        }
        // Кривая не совпала с правилом фабрики — состояние несогласовано, котировки нет.
        _ => None,
    }
}

/// Весовой onSwap. `min` — выпуск 20260115 (WeightedPool.sol L172-L208): баланс входа + 1, нижний баланс входа до и
/// выхода после обмена; None — 20241205 (L150-L176). Принимает: веса, балансы, индексы, сумму, точный ли вход.
/// Возвращает: расчёт пула. None — откат (30 %, нижний баланс, арифметика).
pub fn weighted(
    weights: &[U256],
    bal: &[U256],
    i: usize,
    j: usize,
    a: U256,
    exact_in: bool,
    min: Option<&[U256]>,
) -> Option<U256> {
    // Выпуск v2: баланс входа + 1 и проверка нижнего баланса входа (WeightedPool.sol L174-L180).
    let b_in = match min {
        Some(m) => {
            let b = bal.get(i)?.checked_add(U256::from(1u8))?;
            if b < *m.get(i)? {
                return None;
            }
            b
        }
        None => *bal.get(i)?,
    };
    let b_out = *bal.get(j)?;
    let (w_in, w_out) = (*weights.get(i)?, *weights.get(j)?);
    let (calc, out) = if exact_in {
        let o = weighted_out(b_in, w_in, b_out, w_out, a)?;
        (o, o)
    } else {
        (weighted_in(b_in, w_in, b_out, w_out, a)?, a)
    };
    // Нижний баланс выхода после обмена (L207, вычитание с откатом).
    if let Some(m) = min
        && b_out.checked_sub(out)? < *m.get(j)?
    {
        return None;
    }
    Some(calc)
}

/// Стейблсвап onSwap (StableMath V3; guard — проверка дисбаланса 20260116). None — откат.
fn stable_swap(
    amp: U256,
    bal: &[U256],
    i: usize,
    j: usize,
    a: U256,
    exact_in: bool,
    guard: bool,
) -> Option<U256> {
    let inv = stable::invariant(amp, bal)?;
    let (calc, a_in, a_out) = if exact_in {
        let o = stable::out_given_in(amp, bal, i, j, a, inv)?;
        (o, a, o)
    } else {
        let x = stable::in_given_out(amp, bal, i, j, a, inv)?;
        (x, x, a)
    };
    if guard && !stable::within_imbalance(bal, i, j, a_in, a_out) {
        return None;
    }
    Some(calc)
}

/// QuantAMM onSwap (QuantAMMWeightedPool.sol L301-L340): точный вход — вход ≤ баланс входа · maxTradeSizeRatio (mulDown),
/// WeightedMath, выход ≤ баланс выхода · maxTradeSizeRatio; точный выход — наоборот. Веса — на блоке чтения
/// (getNormalizedWeights L451-L552 считает их тем же calculateBlockNormalisedWeight L558-L572, что onSwap L271-L299,
/// с теми же индексами слов весов и множителей). Откат onSwap при lastInteropTime < lastUpdateTime (L284) —
/// pools/load_balancer_v3_kinds.rs. None — откат.
///
/// Сверено (ревью T433-F2, 20.11.2026): QuantAMMProtocol/QuantAMM-V1@4dca4932
/// pkg/pool-quantamm/contracts/QuantAMMWeightedPool.sol (GitHub, побайтно равен исходнику развёртывания
/// QuantAMMProtocol/deployment@e7867165 v3/tasks/20250429-v3-arbitrum-quantamm build-info QuantAMMWeightedPool.json):
/// пределы до и после WeightedMath — mulDown от живых балансов обмена (L302, L314, L321, L334), WeightedMath /
/// FixedPoint / LogExpMath — побайтно равны balancer-deployments@b55dc2b8 20241205-v3-weighted-pool. Формула и
/// округления совпадают; найдено одно расхождение чтения — откат L284, который getNormalizedWeights не видит (там
/// вычитание в unchecked, L463-L466), — исправлено проверкой времён в pools/load_balancer_v3_kinds.rs.
fn quant_amm(
    weights: &[U256],
    bal: &[U256],
    i: usize,
    j: usize,
    a: U256,
    exact_in: bool,
    max_trade: U256,
) -> Option<U256> {
    let (b_in, b_out) = (*bal.get(i)?, *bal.get(j)?);
    let (w_in, w_out) = (*weights.get(i)?, *weights.get(j)?);
    let (lim_in, lim_out) = (mul_down(b_in, max_trade)?, mul_down(b_out, max_trade)?);
    if exact_in {
        if a > lim_in {
            return None;
        }
        let o = weighted_out(b_in, w_in, b_out, w_out, a)?;
        (o <= lim_out).then_some(o)
    } else {
        if a > lim_out {
            return None;
        }
        let x = weighted_in(b_in, w_in, b_out, w_out, a)?;
        (x <= lim_in).then_some(x)
    }
}

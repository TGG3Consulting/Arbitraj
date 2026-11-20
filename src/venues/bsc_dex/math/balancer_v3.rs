//! Файл: venues/bsc_dex/math/balancer_v3.rs
//! Что делает: решение 441 (T433-F2) — обмен в пуле Balancer V3 так, как его считает хранилище Vault V3 с вызовом
//! onSwap пула (пул без крючков обмена — их проверяет pools/verify_balancer_v3.rs): сумма → 18 знаков с курсом токена,
//! комиссия (точный вход — со входа mulUp; точный выход — поверх mulDivUp), наименьшая сумма обмена, формула пула по
//! правилу его фабрики (весовая — как V2 с powUp V3, стейблсвап — math/balancer_v3_stable.rs), обратно в единицы токена
//! с округлением хранилища.
//! Кто вызывает: math/balancer_math.rs (amount_out / amount_in — ветка состояния с частью V3).
//! От чего зависит: core/types (VaultPoolData, VaultV3, VaultV3Rule, VaultCurve), math/balancer_math.rs (FixedPoint),
//! math/balancer_v3_weighted.rs (powUp V3 и WeightedMath), math/balancer_v3_stable.rs (StableMath V3).
//! Настройки: нет (состояние — pools/load_balancer_v3.rs).
//!
//! Источник (решение 23): balancer/balancer-deployments@b55dc2b8, build-info (исходники развёрнутого кода):
//! - v3/tasks/20241204-v3-vault → contracts/Vault.sol: swap L197-L291 (крючки — только при флагах пула),
//!   _computeAmountGivenScaled18 L326-L346, _swap L368-L430 (комиссия, _ensureValidSwapAmount до и после onSwap,
//!   обратный пересчёт); L1554-L1558 _ensureValidSwapAmount (сумма < _MINIMUM_TRADE_AMOUNT — откат);
//!   v3-solidity-utils/helpers/ScalingHelpers.sol L38-L44 (toScaled18ApplyRateRoundDown), L56-L62 (…RoundUp),
//!   L74-L82 (toRawUndoRateRoundDown = divDown(сумма, множитель·курс)), L94-L102 (…RoundUp = divUp), L212-L220
//!   (computeRateRoundUp); FixedPoint.sol L20-L36 (mulDown, mulUp), L38-L48 (divDown, divUp), L51-L71 (mulDivUp),
//!   L124-L140 (powUp: y = 1 → x, y = 2 → mulUp(x, x), y = 4 → mulUp квадрата, иначе pow + mulUp(pow, 1e4) + 1),
//!   L148-L154 (complement);
//! - весовой: v3/deprecated/20241205-v3-weighted-pool → contracts/WeightedPool.sol onSwap L150-L176;
//!   v3/tasks/20260115-v3-weighted-pool-v2 → WeightedPool.sol onSwap L172-L208 (баланс входа + 1, нижние балансы
//!   _ensureMinimumBalance L378-L397), WeightedMath.sol computeOutGivenExactIn L173-L208, computeInGivenExactOut L219-L255
//!   (MAX_IN_RATIO = MAX_OUT_RATIO = 30 %, L54-L55; у 20241205 — те же формулы, L171-L205 / L216-L255);
//! - стейблсвап: v3/deprecated/20241205-v3-stable-pool → StablePool.sol computeInvariant L139-L148 (ROUND_DOWN — без
//!   +1), onSwap L169-L196; v3/tasks/20260116-v3-stable-pool-v3 → StablePool.sol onSwap L195-L247 (проверка дисбаланса).

use alloy_primitives::U256;

use crate::core::types::pool_vault::{VaultCurve, VaultPoolData, VaultV3, VaultV3Rule};

use super::balancer_math::{div_down, div_up, mul_down, mul_up, one};
use super::balancer_v3_stable as stable;
use super::balancer_v3_weighted::{complement, mul_div_up, weighted_in, weighted_out};

/// Курс с округлением вверх (computeRateRoundUp, ScalingHelpers.sol L212-L220). Ошибок нет.
fn rate_up(r: U256) -> U256 {
    if (r / one()) * one() == r {
        r
    } else {
        r + U256::from(1u8)
    }
}

/// Формула пула по правилу (onSwap). Принимает: кривую, часть V3, индексы, сумму (18 знаков), точный ли вход.
/// Возвращает: выход (точный вход) или вход без комиссии (точный выход), 18 знаков. None — откат пула.
fn on_swap(
    curve: &VaultCurve,
    v3: &VaultV3,
    i: usize,
    j: usize,
    a: U256,
    exact_in: bool,
) -> Option<U256> {
    let bal = &v3.live;
    match (curve, &v3.rule) {
        (VaultCurve::Weighted { weights }, r) if weighted_rule(r) => {
            let min = match &v3.rule {
                VaultV3Rule::WeightedMin { min } => Some(min),
                _ => None,
            };
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
        (VaultCurve::Stable { amp }, r) if !weighted_rule(r) => {
            let inv = stable::invariant(*amp, bal)?;
            let (calc, a_in, a_out) = if exact_in {
                let o = stable::out_given_in(*amp, bal, i, j, a, inv)?;
                (o, a, o)
            } else {
                let x = stable::in_given_out(*amp, bal, i, j, a, inv)?;
                (x, x, a)
            };
            let guard = matches!(v3.rule, VaultV3Rule::StableGuard);
            if guard && !stable::within_imbalance(bal, i, j, a_in, a_out) {
                return None;
            }
            Some(calc)
        }
        // Кривая не совпала с правилом фабрики — состояние несогласовано, котировки нет.
        _ => None,
    }
}

/// Правило весового пула (иначе — стейблсвап). Ошибок нет.
fn weighted_rule(r: &VaultV3Rule) -> bool {
    matches!(r, VaultV3Rule::Weighted | VaultV3Rule::WeightedMin { .. })
}

/// Выход на точный вход (Vault.swap EXACT_IN). Принимает: состояние пула, его часть V3, индексы входа и выхода, вход в
/// единицах токена. Возвращает: выход в единицах токена. None — пауза, индексы, сумма меньше наименьшей, откат пула.
pub fn amount_out(
    d: &VaultPoolData,
    v3: &VaultV3,
    i: usize,
    j: usize,
    amount_in: U256,
) -> Option<U256> {
    if d.paused || i == j || amount_in.is_zero() {
        return None;
    }
    // _computeAmountGivenScaled18 (L333-L337): (сумма · множитель).mulDown(курс).
    let scaled = mul_down(amount_in.checked_mul(*d.scaling.get(i)?)?, *v3.rates.get(i)?)?;
    // _swap L386-L390: комиссия mulUp со входа.
    let net = scaled.checked_sub(mul_up(scaled, d.fee)?)?;
    if net < v3.min_trade {
        return None;
    }
    let out18 = on_swap(&d.curve, v3, i, j, net, true)?;
    if out18 < v3.min_trade {
        return None;
    }
    // L406-L414: toRawUndoRateRoundDown(выход, множитель, computeRateRoundUp(курс)).
    let den = d.scaling.get(j)?.checked_mul(rate_up(*v3.rates.get(j)?))?;
    div_down(out18, den)
}

/// Вход на точный выход (Vault.swap EXACT_OUT). Принимает: состояние, часть V3, индексы, выход в единицах токена.
/// Возвращает: вход в единицах токена с комиссией. None — как amount_out.
pub fn amount_in(
    d: &VaultPoolData,
    v3: &VaultV3,
    i: usize,
    j: usize,
    amount_out: U256,
) -> Option<U256> {
    if d.paused || i == j || amount_out.is_zero() {
        return None;
    }
    // L338-L345: (сумма · множитель).mulUp(computeRateRoundUp(курс выхода)).
    let given = amount_out.checked_mul(*d.scaling.get(j)?)?;
    let scaled = mul_up(given, rate_up(*v3.rates.get(j)?))?;
    if scaled < v3.min_trade {
        return None;
    }
    let in18 = on_swap(&d.curve, v3, i, j, scaled, false)?;
    if in18 < v3.min_trade {
        return None;
    }
    // L419-L427: комиссия mulDivUp(вход, комиссия, 1 − комиссия), затем toRawUndoRateRoundUp(…, курс входа).
    let fee = mul_div_up(in18, d.fee, complement(d.fee))?;
    let total = in18.checked_add(fee)?;
    let den = d.scaling.get(i)?.checked_mul(*v3.rates.get(i)?)?;
    div_up(total, den)
}

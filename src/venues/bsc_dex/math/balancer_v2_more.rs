//! Файл: venues/bsc_dex/math/balancer_v2_more.rs
//! Что делает: решение 473 (T450-03…07) — обмен в пулах Balancer V2 других фабрик (VaultCurve::V2) так, как его
//! считает пул по вызову хранилища: стейблсвап с курсами (composable stable, meta-stable, stable phantom), управляемый
//! весовой пул (managed v1 / v2) и линейный пул. Пулы LBP и investment — кривая Weighted (math/balancer_math.rs).
//! Кто вызывает: math/balancer_math.rs (amount_out / amount_in — ветка VaultCurve::V2).
//! От чего зависит: core/types (VaultPoolData, V2Curve), math/balancer_math.rs (FixedPoint, WeightedMath),
//! math/balancer_stable.rs (StableMath), math/balancer_linear.rs (LinearMath), alloy-primitives.
//! Настройки: нет (состояние — pools/load_balancer_more.rs).
//!
//! Источник (решение 23) — balancer/balancer-deployments@b55dc2b8, build-info задач (выдержки — исходники build-info):
//! - стейблсвап с курсами: BaseGeneralPool.onSwap / _swapGivenIn / _swapGivenOut (20240223-composable-stable-pool-v6
//!   @balancer-labs/v2-pool-utils/contracts/BaseGeneralPool.sol L32-L85: вход − mulUp(вход, комиссия), затем
//!   FixedPoint.mulDown на множитель; выход divDown; точный выход — вход divUp на множитель, затем
//!   divUp(1 − комиссия));
//!   ComposableStablePool._onRegularSwap L279-L298 (BPT выкинут из балансов, StableMath v2 — тот же текст, что
//!   у 20220609-stable-pool-v2, у v1–v6 обмена без BPT один текст); MetaStablePool.sol (20210727) L125-L145 и
//!   StableMath v1 (= 20210624-stable-pool); StablePhantomPool.sol (20211208) L207-L271 (StableMath v1, инвариант с
//!   округлением вверх). Обмен с BPT самого пула (вход / выход ликвидности, _swapWithBpt) не считается — такие пары
//!   пул не даёт (pools/load_balancer_more.rs);
//! - managed: ManagedPool.sol (20221021-managed-pool) _onSwapMinimal L123-L170 (swapEnabled), _onTokenSwap L332-L404
//!   (вход × множитель, затем mulDown(1 − комиссия); точный выход — divUp(1 − комиссия) до деления на множитель),
//!   _checkCircuitBreakersOnRegularSwap L728-L760, _checkOneSidedCircuitBreaker L856-L869, CircuitBreakerLib.sol
//!   L39-L55 (hasCircuitBreakerTripped: цена BPT = Math.div(выпуск × вес, баланс, вверх у верхней границы));
//!   20230411-managed-pool-v2 — тот же расчёт (ManagedPoolAmmLib.checkCircuitBreaker L251); FixedPoint.powUp с точными
//!   степенями 1/2/4 (как у weighted v2 — balancer_math::weighted_out(.., fast = true));
//! - linear: LinearPool._onSwapGeneral (20230410-aave-linear-pool-v5 LinearPool.sol L272-L317; 20211208 / 20220404 /
//!   20220817 — тот же расчёт, отличается только виртуальный выпуск BPT): балансы × множители (обёрнутый — с курсом),
//!   сумма × множитель входа (точный вход) или выхода, LinearMath, выход divDown / вход divUp; комиссия — только внутри
//!   LinearMath (номинальный баланс).

use alloy_primitives::U256;

use crate::core::types::VaultPoolData;
use crate::core::types::pool_vault::V2Curve;

use super::balancer_linear::{self as lin, Bal, Params, Tok};
use super::balancer_math::{div_down, div_up, mul_down, mul_up, one, weighted_in, weighted_out};
use super::balancer_stable;

/// complement: 1 − x, не меньше нуля (FixedPoint.complement).
fn complement(x: U256) -> U256 {
    if x < one() { one() - x } else { U256::ZERO }
}

/// Выход на точный вход. Принимает: состояние, кривую V2, индексы входа и выхода, вход в единицах токена. Возвращает:
/// выход в единицах токена. None — пауза (или swapEnabled = нет), пара с BPT пула, откат формулы или пределов.
pub fn amount_out(d: &VaultPoolData, c: &V2Curve, i: usize, j: usize, a: U256) -> Option<U256> {
    if d.paused || i == j {
        return None;
    }
    match c {
        V2Curve::StableRated { amp, factors, bpt, v2 } => {
            let (f_in, f_out) = (*factors.get(i)?, *factors.get(j)?);
            let (b, ii, jj) = rated_balances(d, factors, *bpt, i, j)?;
            let up = mul_down(a.checked_sub(mul_up(a, d.fee)?)?, f_in)?;
            let out = balancer_stable::out_given_in(*amp, &b, ii, jj, up, *v2)?;
            div_down(out, f_out)
        }
        V2Curve::Managed { weights, lower, upper, supply, bpt } => {
            if usize::from(*bpt) == i || usize::from(*bpt) == j {
                return None;
            }
            let (s_in, s_out) = (*d.scaling.get(i)?, *d.scaling.get(j)?);
            if s_out.is_zero() {
                return None;
            }
            let b_in = d.balances.get(i)?.checked_mul(s_in)?;
            let b_out = d.balances.get(j)?.checked_mul(s_out)?;
            let up = a.checked_mul(s_in)?;
            let net = mul_down(up, complement(d.fee))?;
            let (w_in, w_out) = (*weights.get(i)?, *weights.get(j)?);
            let out = weighted_out((b_in, w_in), (b_out, w_out), net, true)?;
            let (after_in, after_out) = (b_in.checked_add(up)?, b_out.checked_sub(out)?);
            let ok_in = !tripped(*supply, w_in, after_in, *lower.get(i)?, true)?;
            let ok_out = !tripped(*supply, w_out, after_out, *upper.get(j)?, false)?;
            if !(ok_in && ok_out) {
                return None;
            }
            Some(out / s_out)
        }
        V2Curve::Linear { factors, .. } => {
            let (t_in, t_out, bal, p) = linear(d, c, i, j)?;
            let up = mul_down(a, *factors.get(i)?)?;
            let out = lin::out_given_in(t_in, t_out, up, &bal, &p)?;
            div_down(out, *factors.get(j)?)
        }
    }
}

/// Вход на точный выход. Принимает: состояние, кривую V2, индексы, выход в единицах токена. Возвращает: вход в
/// единицах токена с комиссией. None — как amount_out.
pub fn amount_in(d: &VaultPoolData, c: &V2Curve, i: usize, j: usize, a: U256) -> Option<U256> {
    if d.paused || i == j {
        return None;
    }
    match c {
        V2Curve::StableRated { amp, factors, bpt, v2 } => {
            let (f_in, f_out) = (*factors.get(i)?, *factors.get(j)?);
            let (b, ii, jj) = rated_balances(d, factors, *bpt, i, j)?;
            let up = mul_down(a, f_out)?;
            let raw = balancer_stable::in_given_out(*amp, &b, ii, jj, up, *v2)?;
            div_up(div_up(raw, f_in)?, complement(d.fee))
        }
        V2Curve::Managed { weights, lower, upper, supply, bpt } => {
            if usize::from(*bpt) == i || usize::from(*bpt) == j {
                return None;
            }
            let (s_in, s_out) = (*d.scaling.get(i)?, *d.scaling.get(j)?);
            let b_in = d.balances.get(i)?.checked_mul(s_in)?;
            let b_out = d.balances.get(j)?.checked_mul(s_out)?;
            let up = a.checked_mul(s_out)?;
            let (w_in, w_out) = (*weights.get(i)?, *weights.get(j)?);
            let raw = weighted_in((b_in, w_in), (b_out, w_out), up, true)?;
            let with_fee = div_up(raw, complement(d.fee))?;
            let (after_in, after_out) = (b_in.checked_add(with_fee)?, b_out.checked_sub(up)?);
            let ok_in = !tripped(*supply, w_in, after_in, *lower.get(i)?, true)?;
            let ok_out = !tripped(*supply, w_out, after_out, *upper.get(j)?, false)?;
            if !(ok_in && ok_out) || s_in.is_zero() {
                return None;
            }
            // _downscaleUp — ceil(сумма / 10^(18 − decimals)) (FixedPoint.divUp на 1e18·10^(18 − decimals)).
            Some(if with_fee.is_zero() {
                with_fee
            } else {
                (with_fee - U256::from(1u8)) / s_in + U256::from(1u8)
            })
        }
        V2Curve::Linear { factors, .. } => {
            let (t_in, t_out, bal, p) = linear(d, c, i, j)?;
            let up = mul_down(a, *factors.get(j)?)?;
            let raw = lin::in_given_out(t_in, t_out, up, &bal, &p)?;
            div_up(raw, *factors.get(i)?)
        }
    }
}

/// Балансы стейблсвапа × множители FixedPoint (mulDown) без BPT пула и индексы пары среди них. None — пара с BPT,
/// длины не совпали, переполнение.
fn rated_balances(
    d: &VaultPoolData,
    factors: &[U256],
    bpt: Option<u8>,
    i: usize,
    j: usize,
) -> Option<(Vec<U256>, usize, usize)> {
    let skip = bpt.map(usize::from);
    if skip == Some(i) || skip == Some(j) || d.balances.len() != factors.len() {
        return None;
    }
    let mut out = Vec::with_capacity(d.balances.len());
    for (k, (b, f)) in d.balances.iter().zip(factors).enumerate() {
        if Some(k) != skip {
            out.push(mul_down(*b, *f)?);
        }
    }
    let shift = |x: usize| x - usize::from(skip.is_some_and(|s| x > s));
    Some((out, shift(i), shift(j)))
}

/// CircuitBreakerLib.hasCircuitBreakerTripped (L39-L55): граница 0 — нет предела; цена BPT = Math.div(выпуск × вес,
/// баланс, вверх — у верхней границы); нижняя — цена < границы, верхняя — цена > границы. None — откат (деление на
/// ноль, переполнение).
fn tripped(supply: U256, weight: U256, balance: U256, bound: U256, lower: bool) -> Option<bool> {
    if bound.is_zero() {
        return Some(false);
    }
    if balance.is_zero() {
        return None;
    }
    let num = supply.checked_mul(weight)?;
    let price = if !lower && !num.is_zero() {
        (num - U256::from(1u8)) / balance + U256::from(1u8)
    } else {
        num / balance
    };
    Some(if lower { price < bound } else { price > bound })
}

/// Линейный пул: виды токенов входа и выхода, балансы × множители (18 знаков) и параметры (комиссия, цели). None —
/// не линейная кривая, индекс не из пула, виртуальный выпуск < баланса BPT.
fn linear(d: &VaultPoolData, c: &V2Curve, i: usize, j: usize) -> Option<(Tok, Tok, Bal, Params)> {
    let V2Curve::Linear { main, wrapped, bpt, factors, lower, upper, supply } = c else {
        return None;
    };
    let tok = |x: usize| {
        if x == usize::from(*main) {
            Some(Tok::Main)
        } else if x == usize::from(*wrapped) {
            Some(Tok::Wrapped)
        } else if x == usize::from(*bpt) {
            Some(Tok::Bpt)
        } else {
            None
        }
    };
    let up = |x: u8| mul_down(*d.balances.get(usize::from(x))?, *factors.get(usize::from(x))?);
    let bal = Bal {
        main: up(*main)?,
        wrapped: up(*wrapped)?,
        supply: supply.checked_sub(up(*bpt)?)?,
    };
    let p = Params {
        fee: d.fee,
        lower: *lower,
        upper: *upper,
    };
    Some((tok(i)?, tok(j)?, bal, p))
}

//! Файл: venues/bsc_dex/math/balancer_math.rs
//! Что делает: обмен в пуле Balancer V2 (решение 289) так, как его считает контракт пула по вызову хранилища: комиссия
//! со входа (точный вход) или поверх входа (точный выход), множители точности токенов, формула кривой — весовая
//! (WeightedMath) или стейблсвап (math/balancer_stable.rs) — и обратное масштабирование с округлением пула.
//! Кто вызывает: quote/pool_quote.rs (ветка VaultPool), quote/depth_band.rs; тесты на записанных ответах цепи.
//! Решение 441: состояние с частью V3 (VaultPoolData.v3 — пул Balancer V3) считает math/balancer_v3.rs.
//! От чего зависит: core/types (VaultPoolData), math/balancer_log_exp.rs, math/balancer_stable.rs, alloy-primitives.
//! Настройки: нет (состояние — pools/load_balancer.rs).
//!
//! Источник (решение 23) — balancer/balancer-deployments@b55dc2b8, build-info 20210418-weighted-pool (весовой пул) и
//! 20210624-stable-pool (стейблсвап); тексты = Sourcify пулов ACSI (exact_match), выдержки с номерами строк —
//! tests/data/balancer_acsi_2026-11-15/source_excerpt.txt:
//! - FixedPoint.sol L23-L135 (ONE, MAX_POW_RELATIVE_ERROR = 10000, mulDown/mulUp/divDown/divUp, powUp, complement);
//! - WeightedMath.sol L36-L37 (_MAX_IN_RATIO / _MAX_OUT_RATIO = 0.3e18), _calcOutGivenIn L69-L100, _calcInGivenOut
//!   L104-L138;
//! - BaseMinimalSwapInfoPool.sol L54-L89 / BaseGeneralPool.sol (стейбл) L46-L82: точный вход — вход − mulUp(вход,
//!   комиссия), затем ×множитель; выход — divDown на множитель; точный выход — выход ×множитель, вход divUp на
//!   множитель, затем divUp(вход, 1 − комиссия);
//! - BasePool.sol (весовой) L454-L466, L474-L481, L529-L555: множитель = 10^(18 − decimals), _upscale = Math.mul,
//!   _downscaleDown = Math.divDown, _downscaleUp = Math.divUp; у стейбла (BasePool.sol L490-L502, L510-L517,
//!   L574-L616) множитель = 1e18·10^(18 − decimals) с FixedPoint.mulDown / divDown / divUp — те же результаты, поэтому
//!   здесь один множитель 10^(18 − decimals) (VaultPoolData.scaling).
//!   Хранилище передаёт пулу полные балансы (cash + managed: Swaps.sol L378-L379, L423).

use alloy_primitives::U256;

use crate::core::types::{VaultCurve, VaultPoolData};

use super::{balancer_log_exp, balancer_stable};

/// FixedPoint.ONE = 1e18 (FixedPoint.sol L23).
pub fn one() -> U256 {
    super::ten::E18
}

/// mulDown (L45-L50). None — переполнение.
pub fn mul_down(a: U256, b: U256) -> Option<U256> {
    Some(a.checked_mul(b)? / one())
}

/// mulUp (L52-L67). None — переполнение.
pub fn mul_up(a: U256, b: U256) -> Option<U256> {
    let p = a.checked_mul(b)?;
    Some(if p.is_zero() {
        p
    } else {
        (p - U256::from(1u8)) / one() + U256::from(1u8)
    })
}

/// divDown (L69-L80). None — деление на ноль или переполнение.
pub fn div_down(a: U256, b: U256) -> Option<U256> {
    if b.is_zero() {
        return None;
    }
    if a.is_zero() {
        return Some(a);
    }
    Some(a.checked_mul(one())? / b)
}

/// divUp (L82-L99). None — деление на ноль или переполнение.
pub fn div_up(a: U256, b: U256) -> Option<U256> {
    if b.is_zero() {
        return None;
    }
    if a.is_zero() {
        return Some(a);
    }
    Some((a.checked_mul(one())? - U256::from(1u8)) / b + U256::from(1u8))
}

/// powUp (L120-L125): pow + mulUp(pow, MAX_POW_RELATIVE_ERROR = 10000) + 1. None — откат pow или переполнение.
fn pow_up(x: U256, y: U256) -> Option<U256> {
    let raw = balancer_log_exp::pow(x, y)?;
    raw.checked_add(mul_up(raw, U256::from(10_000u32))?)?
        .checked_add(U256::from(1u8))
}

/// complement (L133-L135): 1 − x, не меньше нуля.
fn complement(x: U256) -> U256 {
    if x < one() { one() - x } else { U256::ZERO }
}

/// _MAX_IN_RATIO = _MAX_OUT_RATIO = 0.3e18 (WeightedMath.sol L36-L37).
fn max_ratio() -> U256 {
    U256::from(3u8) * super::ten::E17
}

/// WeightedMath._calcOutGivenIn (L69-L100). Принимает: баланс и вес входа, баланс и вес выхода, вход (всё — 18 знаков,
/// вход уже без комиссии). None — вход больше 30 % баланса (MAX_IN_RATIO) или откат арифметики.
pub fn weighted_out(b_in: U256, w_in: U256, b_out: U256, w_out: U256, a_in: U256) -> Option<U256> {
    if a_in > mul_down(b_in, max_ratio())? {
        return None;
    }
    let base = div_up(b_in, b_in.checked_add(a_in)?)?;
    let power = pow_up(base, div_down(w_in, w_out)?)?;
    mul_down(b_out, complement(power))
}

/// WeightedMath._calcInGivenOut (L104-L138). Принимает: баланс и вес входа, баланс и вес выхода, выход (18 знаков).
/// Возвращает: вход без комиссии. None — выход больше 30 % баланса (MAX_OUT_RATIO) или откат арифметики.
pub fn weighted_in(b_in: U256, w_in: U256, b_out: U256, w_out: U256, a_out: U256) -> Option<U256> {
    if a_out > mul_down(b_out, max_ratio())? {
        return None;
    }
    let base = div_up(b_out, b_out.checked_sub(a_out)?)?;
    let power = pow_up(base, div_up(w_out, w_in)?)?;
    mul_up(b_in, power.checked_sub(one())?)
}

/// Выход пула на точный вход (вызов хранилища onSwap GIVEN_IN). Принимает: состояние пула, индексы входа и выхода
/// среди токенов пула, вход в единицах токена. Возвращает: выход в единицах токена. None — пауза, индексы вне пула,
/// откат формулы (вход > 30 % у весового, несходимость у стейблсвапа) или переполнение.
pub fn amount_out(d: &VaultPoolData, i: usize, j: usize, amount_in: U256) -> Option<U256> {
    // Решение 441: пул хранилища Balancer V3 — расчёт хранилища V3 (курсы, наименьшая сумма, правило фабрики).
    if let Some(v3) = &d.v3 {
        return super::balancer_v3::amount_out(d, v3, i, j, amount_in);
    }
    let (s_in, s_out) = (*d.scaling.get(i)?, *d.scaling.get(j)?);
    if d.paused || i == j || s_out.is_zero() {
        return None;
    }
    let net = amount_in.checked_sub(mul_up(amount_in, d.fee)?)?;
    let a = net.checked_mul(s_in)?;
    let out = match &d.curve {
        VaultCurve::Weighted { weights } => weighted_out(
            d.balances.get(i)?.checked_mul(s_in)?,
            *weights.get(i)?,
            d.balances.get(j)?.checked_mul(s_out)?,
            *weights.get(j)?,
            a,
        )?,
        VaultCurve::Stable { amp } => balancer_stable::out_given_in(*amp, &upscaled(d)?, i, j, a)?,
    };
    Some(out / s_out)
}

/// Вход пула на точный выход (onSwap GIVEN_OUT). Принимает: состояние, индексы входа и выхода, выход в единицах
/// токена. Возвращает: вход в единицах токена с комиссией. None — как amount_out.
pub fn amount_in(d: &VaultPoolData, i: usize, j: usize, amount_out: U256) -> Option<U256> {
    // Решение 441: пул хранилища Balancer V3 (math/balancer_v3.rs).
    if let Some(v3) = &d.v3 {
        return super::balancer_v3::amount_in(d, v3, i, j, amount_out);
    }
    let (s_in, s_out) = (*d.scaling.get(i)?, *d.scaling.get(j)?);
    if d.paused || i == j || s_in.is_zero() {
        return None;
    }
    let a = amount_out.checked_mul(s_out)?;
    let raw = match &d.curve {
        VaultCurve::Weighted { weights } => weighted_in(
            d.balances.get(i)?.checked_mul(s_in)?,
            *weights.get(i)?,
            d.balances.get(j)?.checked_mul(s_out)?,
            *weights.get(j)?,
            a,
        )?,
        VaultCurve::Stable { amp } => balancer_stable::in_given_out(*amp, &upscaled(d)?, i, j, a)?,
    };
    // _downscaleUp — Math.divUp (не FixedPoint): ceil(raw / множитель).
    let down = if raw.is_zero() {
        raw
    } else {
        (raw - U256::from(1u8)) / s_in + U256::from(1u8)
    };
    div_up(down, complement(d.fee))
}

/// Балансы пула × множители точности (18 знаков). None — переполнение или длины не совпали.
fn upscaled(d: &VaultPoolData) -> Option<Vec<U256>> {
    if d.balances.len() != d.scaling.len() {
        return None;
    }
    d.balances
        .iter()
        .zip(&d.scaling)
        .map(|(b, s)| b.checked_mul(*s))
        .collect()
}

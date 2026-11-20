//! Файл: venues/bsc_dex/pools/load_balancer_more_parts.rs
//! Что делает: решение 473 (T450-03…07) — части разбора пачки пула Balancer V2 вида dex.balancer_pool: множители
//! стейблсвапа с курсом на блоке чтения (composable stable, meta-stable, stable phantom), кривая managed (веса, границы
//! цены BPT, фактический выпуск) и кривая linear (множители с курсом обёрнутого, цели, виртуальный выпуск). Вынесено из
//! load_balancer_more_curve.rs (предел размера файла).
//! Кто вызывает: pools/load_balancer_more_curve.rs (decode).
//! От чего зависит: core/types (VaultCurve, V2Curve), abi_balancer_more.rs, load_balancer_more.rs (rated),
//! load_balancer_more_curve.rs (Cur), load_lb.rs (decode_time), math/balancer_math.rs (mulDown, ONE).
//! Настройки: нет.
//!
//! Источник — шапка load_balancer_more_curve.rs.

use alloy_primitives::{Address, U256};

use crate::core::types::VaultCurve;
use crate::core::types::pool_vault::V2Curve;

use super::super::math::balancer_math::mul_down;
use super::abi_balancer_more::{
    IComposable as C, ILinear as L, IMetaStable as M, IPhantom as Ph, IRateProvider as R,
    ISmartWeighted as W,
};
use super::balancer_pool_type::PoolType;
use super::load_balancer_more::rated;
use super::load_balancer_more_curve::Cur;
use super::load_balancer_more_fixed::More;

/// Время блока (Multicall3.getCurrentBlockTimestamp) — следующий ответ пачки. Ошибки (текст): откат, нет ответа, не по
/// ABI.
fn take_time(c: &mut Cur) -> Result<u64, String> {
    let i = c.at;
    c.at += 1;
    match c.got.get(i) {
        Some(Ok(b)) => super::load_lb::decode_time(b),
        Some(Err(r)) => Err(format!("вызов {i} (время блока) откатился: {r}")),
        None => Err(format!("нет ответа на вызов {i} (время блока)")),
    }
}

/// Множители стейблсвапа с курсом (composable stable, meta-stable, stable phantom): на блоке чтения и — если у какого-то
/// токена кеш истекает к следующей секунде (expires < время чтения + 1; пул сравнивает block.timestamp исполнения,
/// целые секунды, монотонно) — второй набор с курсом поставщика у таких токенов. Курс поставщика нужен только там, где
/// кеш истёк или истекает: его откат или переполнение у токена с действующим кешем пул не отключают. Возвращает:
/// (множители, второй набор или None). Ошибки (текст): ответы, нет времени блока, нужный курс поставщика откатился или
/// больше предела кеша.
pub fn factors(m: &More, c: &mut Cur) -> Result<(Vec<U256>, Option<Vec<U256>>), String> {
    if m.ty == PoolType::Phantom {
        // Кеш phantom обновляется после расчёта множителей обмена — обмен по getScalingFactor на блоке.
        let f: Result<Vec<U256>, String> =
            m.tokens.iter().map(|_| c.take::<Ph::getScalingFactorCall>()).collect();
        return Ok((f?, None));
    }
    let cached = if m.ty == PoolType::Composable {
        Some(c.take::<C::getScalingFactorsCall>()?)
    } else {
        None
    };
    let mut fresh = Vec::new();
    for (t, _) in rated(m) {
        let (rate, expires) = if m.ty == PoolType::Composable {
            let r = c.take::<C::getTokenRateCacheCall>()?;
            (r.rate, r.expires)
        } else {
            let r = c.take::<M::getPriceRateCacheCall>()?;
            (r.rate, r.expires)
        };
        fresh.push((t, rate, expires, c.take::<R::getRateCall>()));
    }
    let now = if fresh.is_empty() {
        U256::ZERO
    } else if m.time.is_some() {
        U256::from(take_time(c)?)
    } else {
        return Err("dex.multicall3 не задана — нет времени блока для кеша курсов пула".into());
    };
    // Предел курса в кеше: 96 бит (composable, PriceRateCache.sol L41) / 128 бит (meta-stable, L612).
    let bits: usize = if m.ty == PoolType::Composable { 96 } else { 128 };
    let live = |k: usize, t: &Address, r: &Result<U256, String>| -> Result<U256, String> {
        let r = r.clone().map_err(|w| format!("getRate поставщика токена {t}: {w}"))?;
        if r >> bits != U256::ZERO {
            let w = "обмен откатится (PRICE_RATE_OVERFLOW)";
            return Err(format!("курс {r} токена {t} ≥ 2^{bits} — {w}"));
        }
        mul_down(m.dec[k], r).ok_or_else(|| "множитель с курсом не помещается".to_string())
    };
    let (mut out, mut alt, mut near) = (Vec::new(), Vec::new(), false);
    for (k, t) in m.tokens.iter().enumerate() {
        let own = cached.as_ref().map(|s| s.get(k).copied());
        let (f, g) = match (fresh.iter().find(|x| x.0 == *t), own) {
            // Кеш истёк — пул до расчёта берёт курс поставщика (и пишет его в кеш: сверх предела — откат).
            (Some((_, _, expires, r)), _) if now > *expires => {
                let f = live(k, t, r)?;
                (f, f)
            }
            (Some((_, cache, expires, r)), own) => {
                // Кеш действует: composable — множитель getScalingFactors, meta-stable — decimals·mulDown(кеш).
                let f = match own {
                    Some(s) => s.ok_or("getScalingFactors: не по числу токенов")?,
                    None => mul_down(m.dec[k], *cache).ok_or("множитель с курсом не помещается")?,
                };
                // Истекает к следующей секунде — у блока исполнения курс может быть уже поставщика.
                if *expires < now + U256::from(1u8) {
                    near = true;
                    (f, live(k, t, r)?)
                } else {
                    (f, f)
                }
            }
            (None, Some(s)) => {
                let f = s.ok_or("getScalingFactors: не по числу токенов")?;
                (f, f)
            }
            (None, None) => (m.dec[k], m.dec[k]),
        };
        out.push(f);
        alt.push(g);
    }
    Ok((out, near.then_some(alt)))
}

/// Кривая managed: веса (у BPT — 0), границы цены BPT в единицах хранения пула, фактический выпуск. Ошибки (текст):
/// ответы, весов не по числу токенов без BPT.
pub fn managed(
    m: &More,
    c: &mut Cur,
    w: Vec<U256>,
    scaling: &[U256],
) -> Result<VaultCurve, String> {
    let supply = c.take::<W::getActualSupplyCall>()?;
    let b = m.bpt.ok_or("managed: нет BPT среди токенов")?;
    if w.len() + 1 != m.tokens.len() {
        return Err(format!("весов {} при токенах {} (с BPT)", w.len(), m.tokens.len()));
    }
    let n = m.tokens.len();
    let mut weights = vec![U256::ZERO; n];
    let (mut lower, mut upper) = (weights.clone(), weights.clone());
    let mut wi = w.into_iter();
    for k in (0..n).filter(|k| *k != b) {
        weights[k] = wi.next().ok_or("весов меньше токенов")?;
        let s = c.take::<W::getCircuitBreakerStateCall>()?;
        let sc = *scaling.get(k).filter(|x| !x.is_zero()).ok_or("нет множителя токена")?;
        lower[k] = s.lowerBptPriceBound / sc;
        upper[k] = s.upperBptPriceBound / sc;
    }
    let bpt = b as u8;
    let cv = V2Curve::Managed {
        weights,
        lower,
        upper,
        supply,
        bpt,
    };
    Ok(VaultCurve::V2(Box::new(cv)))
}

/// Кривая linear: множители (обёрнутый — с курсом), цели, выпуск BPT (v1 — 2^112 − 1, LinearPool.sol L67). Ошибки
/// (текст): ответы, множитель не помещается.
pub fn linear(m: &More, c: &mut Cur, t: (U256, U256), bal: &[U256]) -> Result<VaultCurve, String> {
    let rate = c.take::<L::getWrappedTokenRateCall>()?;
    let supply = if m.ty == PoolType::Linear {
        c.take::<L::totalSupplyCall>()?
    } else {
        (U256::from(1u8) << 112usize) - U256::from(1u8)
    };
    let b = m.bpt.ok_or("linear: нет BPT среди токенов")?;
    if bal.len() != 3 {
        return Err(format!("linear: токенов {} ≠ 3", bal.len()));
    }
    let mut factors = m.dec.clone();
    let w = "множитель обёрнутого не помещается";
    factors[m.wrapped] = mul_down(m.dec[m.wrapped], rate).ok_or(w)?;
    factors[b] = super::super::math::balancer_math::one();
    let (main, wrapped, bpt) = (m.main as u8, m.wrapped as u8, b as u8);
    let (lower, upper) = t;
    let cv = V2Curve::Linear {
        main,
        wrapped,
        bpt,
        factors,
        lower,
        upper,
        supply,
    };
    Ok(VaultCurve::V2(Box::new(cv)))
}

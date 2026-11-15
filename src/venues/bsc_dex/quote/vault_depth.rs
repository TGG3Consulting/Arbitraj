//! Файл: venues/bsc_dex/quote/vault_depth.rs
//! Что делает: глубина пула в общем хранилище (Balancer V2, решение 289) по направлению в полосе цены (решение 73) —
//! без комиссии пула (чистый объём, решение Тиграна к 73): средняя цена исполнения не выходит за полосу от цены малой
//! пробы (покупка — не выше p0·(1+b), продажа — не ниже p0·(1−b)). Поиск суммы — делением отрезка пополам по той же
//! формуле, что котировка (math/balancer_math.rs с нулевой комиссией). Весовой пул не берёт вход больше 30 % баланса
//! (MAX_IN_RATIO) — тогда глубина до этого предела и full = false.
//! Кто вызывает: quote/depth_band.rs (ветка VaultPool).
//! От чего зависит: core/types (VaultPoolData), math/balancer_math.rs.
//! Настройки: нет (полосу даёт вызывающий).

use alloy_primitives::U256;

use crate::core::types::VaultPoolData;

use super::super::math::balancer_math;

/// Шагов деления пополам (точность — 2^-64 от верхней границы поиска); не настройка — свойство алгоритма поиска.
const STEPS: u32 = 64;
/// Доля баланса входа для пробы цены: 1/10^6 (малая сумма, по которой берётся цена p0 — сдвиг цены пула от неё много
/// меньше полосы dex.depth_band_pct). Свойство алгоритма оценки, не порог торговли. Её же берёт spot.rs (цена для
/// размера пробы), stable_depth.rs и solidly_depth.rs (старт поиска глубины, решение 304).
pub const PROBE_DIV: u64 = 1_000_000;
/// Потолок поиска глубины стейблсвапа — во столько раз больше баланса входа. Предела входа у StableMath в исходнике нет
/// (MAX_IN_RATIO 30 % — только у весового, WeightedMath.sol L36), поэтому граница поиска — свойство алгоритма (как 64
/// шага в stable_depth.rs, StableMath.sol L29-L33 — только пределы A и числа токенов): на потолке средняя цена ещё в
/// полосе — глубина до потолка и full = false.
const STABLE_CAP_BALANCES: u8 = 10;

/// Одно направление. Принимает: состояние пула, индексы входа и выхода, полосу b (0..1), покупка ли (вход — встречный
/// токен). Возвращает: (сумма встречного токена: покупка — вход, продажа — выход; полоса прочитана целиком) или None —
/// пул пуст / на паузе / расчёт невозможен.
pub fn side(d: &VaultPoolData, i: usize, j: usize, b: f64, buy: bool) -> Option<(U256, bool)> {
    if !(b > 0.0 && b < 1.0) || d.paused {
        return None;
    }
    let mut nf = d.clone();
    nf.fee = U256::ZERO;
    let bal_in = *d.balances.get(i)?;
    let probe = bal_in / U256::from(PROBE_DIV);
    if probe.is_zero() {
        return Some((U256::ZERO, true));
    }
    let p_out = balancer_math::amount_out(&nf, i, j, probe)?;
    if p_out.is_zero() {
        return Some((U256::ZERO, true));
    }
    let bp = (b * 10_000.0) as u64;
    let lim = U256::from(if buy { 10_000 + bp } else { 10_000 - bp });
    let ten_k = U256::from(10_000u32);
    // Средняя цена «встречный за монету» в полосе? Покупка: in/out ≤ (p_in/p_out)·lim/1e4; продажа: out/in ≥ …·lim/1e4.
    let within = |x: U256| -> Option<bool> {
        let o = balancer_math::amount_out(&nf, i, j, x)?;
        if o.is_zero() {
            return Some(false);
        }
        Some(if buy {
            x.checked_mul(p_out)?.checked_mul(ten_k)? <= probe.checked_mul(o)?.checked_mul(lim)?
        } else {
            o.checked_mul(probe)?.checked_mul(ten_k)? >= p_out.checked_mul(x)?.checked_mul(lim)?
        })
    };
    // Верхняя граница поиска: весовой — 30 % баланса входа (больше пул откатит), стейблсвап — STABLE_CAP_BALANCES балансов.
    let cap = match &d.curve {
        crate::core::types::VaultCurve::Weighted { .. } => balancer_math::mul_down(
            bal_in,
            U256::from(3u8) * U256::from(10u8).pow(U256::from(17u8)),
        )?,
        crate::core::types::VaultCurve::Stable { .. } => {
            bal_in.checked_mul(U256::from(STABLE_CAP_BALANCES))?
        }
    };
    if within(cap).unwrap_or(false) {
        let o = balancer_math::amount_out(&nf, i, j, cap)?;
        return Some((if buy { cap } else { o }, false));
    }
    let (mut lo, mut hi) = (probe, cap);
    if !within(lo).unwrap_or(false) {
        return Some((U256::ZERO, true));
    }
    for _ in 0..STEPS {
        if hi <= lo + U256::from(1u8) {
            break;
        }
        let mid = lo + (hi - lo) / U256::from(2u8);
        if within(mid).unwrap_or(false) {
            lo = mid
        } else {
            hi = mid
        }
    }
    let o = balancer_math::amount_out(&nf, i, j, lo)?;
    Some((if buy { lo } else { o }, true))
}

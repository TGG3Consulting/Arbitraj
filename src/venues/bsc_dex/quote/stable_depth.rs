//! Файл: venues/bsc_dex/quote/stable_depth.rs
//! Что делает: глубина пула StableSwap (решение 122 F) по направлению в полосе цены (решение 73) — по СРЕДНЕЙ цене
//! исполнения, как у стабильного Solidly и DODO PMM (quote/solidly_depth.rs, pmm_depth.rs): сколько встречного токена
//! пул примет (покупка) или отдаст (продажа), пока средняя цена монеты не отойдёт от маржинальной на полосу b. Без
//! комиссии пула (чистый объём, решение 73). Маржинальная цена — производная инварианта: цена монеты i в монете j =
//! (∂F/∂x_i)/(∂F/∂x_j) = (Ann + Dr/x_i)/(Ann + Dr/x_j), Dr = D^(n+1)/(n^n·Πx) (F — инвариант StableSwap.vy get_D).
//! Монотонность → удвоение объёма + двоичный поиск границы.
//! Кто вызывает: quote/depth_band.rs (ветка StableSwap).
//! От чего зависит: math/stableswap_v1.rs, math/full_math.rs.
//! Настройки: полосу даёт вызывающий (dex.depth_band_pct).

use alloy_primitives::U256;

use super::super::math::full_math::mul_div;
use super::super::math::stableswap_v1::{Template, exchange_out_t, get_d_t, precision, xp};

/// Максимум удвоений при поиске верхней границы объёма (предел итераций, не порог торговли).
const MAX_DOUBLE: u32 = 192;
/// Шагов двоичного поиска границы полосы.
const BISECT: u32 = 64;

/// Маржинальная цена монеты i в монете j (нормализованные 18 знаков, ·1e18). None — пустой пул, переполнение.
fn price(xp: &[U256], amp: U256, i: usize, j: usize, t: &Template) -> Option<U256> {
    let n = U256::from(xp.len());
    let d = get_d_t(xp, amp, t)?;
    let mut dr = d;
    for x in xp {
        let den = x.checked_mul(n)?;
        if den.is_zero() {
            return None;
        }
        dr = dr.checked_mul(d)? / den;
    }
    let ann = amp.checked_mul(n)?;
    let (xi, xj) = (*xp.get(i)?, *xp.get(j)?);
    // (Ann/AP + Dr/x_i)/(Ann/AP + Dr/x_j) = (Ann·x_i·x_j + AP·Dr·x_j) / (Ann·x_i·x_j + AP·Dr·x_i) (решение 122 F, 3:
    // точность A шаблона; при AP = 1 — прежняя формула).
    let ap = t.amp_precision;
    let base = ann.checked_mul(xi)?.checked_mul(xj)?;
    let num = base.checked_add(ap.checked_mul(dr)?.checked_mul(xj)?)?;
    let den = base.checked_add(ap.checked_mul(dr)?.checked_mul(xi)?)?;
    if den.is_zero() {
        return None;
    }
    mul_div(num, precision(), den)
}

/// Глубина одной стороны пула в полосе b (доля 0..1). Принимает: балансы, rates, A, индекс монеты и встречного токена,
/// полосу, направление (buy — встречный токен в пул), шаблон пула. Возвращает: (сумма во ВСТРЕЧНОМ токене, полоса
/// целиком). None — пустой пул или расчёт невозможен.
pub fn side(
    balances: &[U256],
    rates: &[U256],
    amp: U256,
    (ic, iu): (usize, usize),
    b: f64,
    buy: bool,
    t: &Template,
) -> Option<(U256, bool)> {
    if !(b > 0.0 && b < 1.0) {
        return None;
    }
    let x = xp(balances, rates)?;
    let p = price(&x, amp, ic, iu, t)?;
    if p.is_zero() {
        return None;
    }
    let bp = (b * 10_000.0) as u64;
    let limit = if buy {
        mul_div(p, U256::from(10_000 + bp), U256::from(10_000u64))?
    } else {
        mul_div(p, U256::from(10_000 - bp), U256::from(10_000u64))?
    };
    let norm = |amount: U256, k: usize| Some(amount.checked_mul(*rates.get(k)?)? / precision());
    // Встречный и монетный объёмы на входе v (покупка — вход встречный, продажа — вход монета), без комиссии.
    let amounts = |v: U256| -> Option<(U256, U256)> {
        if buy {
            Some((
                v,
                exchange_out_t(iu, ic, v, balances, rates, amp, U256::ZERO, t)?,
            ))
        } else {
            Some((
                exchange_out_t(ic, iu, v, balances, rates, amp, U256::ZERO, t)?,
                v,
            ))
        }
    };
    // В полосе ли средняя цена на входе v: Some(None) — выход округлился в ноль (объём мал, растим дальше).
    let within = |v: U256| -> Option<Option<bool>> {
        let (cnt, coin) = amounts(v)?;
        let coin_n = norm(coin, ic)?;
        if coin_n.is_zero() {
            return Some(None);
        }
        let avg = mul_div(norm(cnt, iu)?, precision(), coin_n)?;
        Some(Some(if buy { avg <= limit } else { avg >= limit }))
    };
    // Удвоение: сначала до первого ненулевого выхода, затем, пока в полосе.
    let mut lo = U256::ZERO;
    let mut hi = U256::from(1u8);
    let mut found_hi = false;
    for _ in 0..MAX_DOUBLE {
        match within(hi).unwrap_or(Some(false)) {
            None => {}
            Some(true) => lo = hi,
            Some(false) => {
                found_hi = true;
                break;
            }
        }
        hi = hi.checked_mul(U256::from(2u8))?;
    }
    if lo.is_zero() {
        return Some((U256::ZERO, true));
    }
    if found_hi {
        for _ in 0..BISECT {
            let mid = lo.checked_add(hi.checked_sub(lo)? / U256::from(2u8))?;
            if mid == lo {
                break;
            }
            if within(mid).unwrap_or(Some(false)).unwrap_or(false) {
                lo = mid;
            } else {
                hi = mid;
            }
        }
    }
    Some((amounts(lo)?.0, found_hi))
}

//! Файл: venues/bsc_dex/quote/wombat_depth.rs
//! Что делает: решение 484 — глубина пула Wombat Exchange по направлению в полосе цены (решение 73) по СРЕДНЕЙ цене
//! исполнения, как у StableSwap (quote/stable_depth.rs): сколько встречного токена пул примет (покупка) или отдаст
//! (продажа), пока средняя цена монеты не отойдёт от опорной на полосу b. Без комиссии пула (чистый объём, решение 73):
//! идеальный выход PoolV2._quoteFrom до haircut и комиссии за покрытие (math/wombat.rs ideal). Опорная цена — средняя
//! цена пробы (доля 1/PROBE_DIV баланса входа, как у Balancer и StableSwap — решение 304): у инварианта Wombat
//! отдельной формулы маржинальной цены в контракте нет; это оценка размера, а не сверяемая величина. Монотонность —
//! удвоение объёма и двоичный поиск границы.
//! Кто вызывает: quote/depth_band.rs (ветка StableSwap со StableData.wombat).
//! От чего зависит: math/wombat.rs (ideal), math/wombat_core.rs (to_wad, from_wad), math/full_math.rs,
//! quote/vault_depth.rs (PROBE_DIV).
//! Настройки: полосу даёт вызывающий (dex.depth_band_pct).

use alloy_primitives::U256;

use crate::core::types::StableData;

use super::super::math::full_math::mul_div;
use super::super::math::wombat::ideal;
use super::super::math::wombat_core::{from_wad, to_wad, wad};
use super::vault_depth::PROBE_DIV;

/// Максимум удвоений при поиске верхней границы объёма (предел итераций, не порог торговли).
const MAX_DOUBLE: u32 = 192;
/// Шагов двоичного поиска границы полосы.
const BISECT: u32 = 64;

/// Выход без комиссии в единицах монеты выхода. None — откат идеального расчёта.
fn out(d: &StableData, i: usize, j: usize, v: U256) -> Option<U256> {
    let w = d.wombat.as_deref()?;
    let x = ideal(w, d.amp, i, j, to_wad(v, *w.decimals.get(i)?)?)?;
    from_wad(x, *w.decimals.get(j)?)
}

/// Глубина одной стороны пула в полосе b (доля 0..1). Принимает: состояние пула (StableData с частью Wombat), индекс
/// монеты и встречного токена, полосу, направление (buy — встречный токен в пул). Возвращает: (сумма во ВСТРЕЧНОМ
/// токене, полоса целиком); ноль — проба 0 или выход пробы 0. None — нет части Wombat, полоса вне 0..1, расчёт
/// невозможен.
pub fn side(d: &StableData, (ic, iu): (usize, usize), b: f64, buy: bool) -> Option<(U256, bool)> {
    if !(b > 0.0 && b < 1.0) {
        return None;
    }
    let w = d.wombat.as_deref()?;
    let norm = |amount: U256, k: usize| to_wad(amount, *w.decimals.get(k)?);
    // (встречный, монета) на входе v без комиссии: покупка — вход встречный, продажа — вход монета.
    let amounts = |v: U256| -> Option<(U256, U256)> {
        if buy {
            Some((v, out(d, iu, ic, v)?))
        } else {
            Some((out(d, ic, iu, v)?, v))
        }
    };
    // Средняя цена монеты во встречном (18 знаков); None — выход 0 или откат.
    let avg = |v: U256| -> Option<U256> {
        let (cnt, coin) = amounts(v)?;
        let coin_n = norm(coin, ic)?;
        if coin_n.is_zero() {
            return None;
        }
        mul_div(norm(cnt, iu)?, wad(), coin_n)
    };
    let bal_in = *d.balances.get(if buy { iu } else { ic })?;
    let probe = bal_in / U256::from(PROBE_DIV);
    if probe.is_zero() {
        return Some((U256::ZERO, true));
    }
    let Some(p) = avg(probe) else {
        return Some((U256::ZERO, true));
    };
    let bp = (b * 10_000.0) as u64;
    let limit = if buy {
        mul_div(p, U256::from(10_000 + bp), U256::from(10_000u64))?
    } else {
        mul_div(p, U256::from(10_000 - bp), U256::from(10_000u64))?
    };
    let within = |v: U256| avg(v).is_some_and(|a| if buy { a <= limit } else { a >= limit });
    let (mut lo, mut hi, mut found_hi) = (probe, probe, false);
    for _ in 0..MAX_DOUBLE {
        hi = hi.checked_mul(U256::from(2u8))?;
        if !within(hi) {
            found_hi = true;
            break;
        }
        lo = hi;
    }
    if found_hi {
        for _ in 0..BISECT {
            let mid = lo.checked_add(hi.checked_sub(lo)? / U256::from(2u8))?;
            if mid == lo {
                break;
            }
            if within(mid) {
                lo = mid;
            } else {
                hi = mid;
            }
        }
    }
    Some((amounts(lo)?.0, found_hi))
}

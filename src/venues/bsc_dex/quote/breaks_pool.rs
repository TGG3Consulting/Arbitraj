//! Файл: venues/bsc_dex/quote/breaks_pool.rs
//! Что делает: решение 344 (задача t159) — изломы кривой одного пула по объёму монеты (минимальные единицы): объёмы
//! монеты, на которых у пула меняется кусок кривой обмена в нужном направлении. Это точки подбора объёма, не
//! данные решения: значения считаются той же математикой пула, где она есть, и без комиссии там, где излом — граница
//! ёмкости (бины). Виды:
//! - V3 (PancakeSwap / Uniswap V3, Slipstream) и Cl (Infinity CL, Uniswap V4): объём монеты до каждого следующего
//!   инициализированного тика по направлению обмена — обмен до предела цены тика (math/swap_cl.rs swap_to — та же
//!   математика и комиссия, что котировка, pool_quote.rs); продажа — вход, покупка — выход;
//! - Bin (Infinity Bin) и LbBin (Liquidity Book V2.1): накопленная ёмкость бинов от активного по направлению обмена
//!   (выход — резерв бина; вход — резерв выхода по цене бина, math/bin_price.rs price_from_id, Y за 1 X ·2^128);
//! - OraclePmm (WooFi, Metric, iZiSwap, Biswap V3): точки лесенки цен пула нужного направления (pool_quote.rs ветка
//!   OraclePmm, quote/pmm_ladder.rs) — на них средняя цена меняет отрезок интерполяции.
//!
//! Прочие виды (V2, Solidly, StableSwap, DMM, Balancer — гладкие кривые без изломов; Algebra, Elastic — нет общей
//! функции «обмен до тика»; DODO, SmarDex, Maverick — переходы не выведены, docs/notes/t159-…) — изломов нет (пусто).
//! Кто вызывает: quote/breaks.rs.
//! От чего зависит: math/swap_cl.rs, math/tick_math.rs, math/bin_price.rs, math/full_math.rs, quote/pool_quote.rs
//! (swap_fee), pools/registry.rs (Entry), core/types (PoolKind, PoolState).
//! Настройки: нет (число изломов и наибольший объём даёт вызывающий).

use alloy_primitives::{Address, U256};

use crate::core::types::{PoolKind, PoolState};

use super::super::math::bin_price::{SCALE_OFFSET, price_from_id};
use super::super::math::full_math::mul_div;
use super::super::math::swap_cl::{self, ClState, Version};
use super::super::math::tick_math::sqrt_ratio_at_tick;
use super::super::pools::bins::BinMap;
use super::super::pools::registry::Entry;
use super::pool_quote::swap_fee;

/// Что делает: изломы пула. Принимает: запись пула, адрес монеты, продажа ли (монета на входе), наибольший объём
/// (единиц монеты), сколько изломов. Возвращает: объёмы монеты по возрастанию (не больше max). Ошибок нет: нет данных
/// или вида без изломов — пусто.
pub fn pool(e: &Entry, coin: Address, sell: bool, max: U256, n: u32) -> Vec<U256> {
    let coin0 = coin == e.pool.token0;
    // Направление обмена: продажа — вход монета; покупка — вход встречный токен.
    let zfo = if sell { coin0 } else { !coin0 };
    let n = n as usize;
    match (&e.pool.state, e.pool.kind) {
        (
            PoolState::V3 {
                sqrt_price_x96,
                liquidity,
                tick,
            },
            PoolKind::V3,
        ) => match e.pool.fee_pips {
            Some(fee) => cl(
                e,
                *sqrt_price_x96,
                *tick,
                *liquidity,
                fee,
                Version::V3,
                zfo,
                sell,
                max,
                n,
            ),
            None => Vec::new(),
        },
        (
            PoolState::Cl {
                sqrt_price_x96,
                tick,
                liquidity,
                protocol_fee,
                lp_fee,
            },
            PoolKind::Cl,
        ) => {
            let fee = swap_fee(*protocol_fee, *lp_fee, zfo);
            cl(
                e,
                *sqrt_price_x96,
                *tick,
                *liquidity,
                fee,
                Version::V4,
                zfo,
                sell,
                max,
                n,
            )
        }
        (PoolState::Bin { active_id, .. }, PoolKind::Bin) => {
            match (e.bins.as_ref(), e.pool.bin_step) {
                (Some(b), Some(step)) => bins(b, *active_id, step, coin0, sell, max, n),
                _ => Vec::new(),
            }
        }
        (PoolState::LbBin(d), PoolKind::LbBin) => match e.bins.as_ref() {
            // X — token0 записи, если x_is_token0 (pool_quote.rs ветка LbBin).
            Some(b) => bins(
                b,
                d.active_id,
                d.bin_step,
                coin0 == d.x_is_token0,
                sell,
                max,
                n,
            ),
            None => Vec::new(),
        },
        (PoolState::OraclePmm(d), PoolKind::OraclePmm) => {
            let points = if zfo == d.base_is_token0 {
                &d.ladder_base_in
            } else {
                &d.ladder_quote_in
            };
            points
                .iter()
                .map(|p| if sell { p.amount_in } else { p.amount_out })
                .filter(|v| !v.is_zero() && *v <= max)
                .take(n)
                .collect()
        }
        _ => Vec::new(),
    }
}

/// Объёмы монеты до следующих инициализированных тиков (обмен точным входом до предела цены тика). Ошибок нет.
#[allow(clippy::too_many_arguments)]
fn cl(
    e: &Entry,
    sqrt_price: U256,
    tick: i32,
    liquidity: u128,
    fee: u32,
    version: Version,
    zfo: bool,
    sell: bool,
    max: U256,
    n: usize,
) -> Vec<U256> {
    let Some(map) = e.ticks.as_ref() else {
        return Vec::new();
    };
    let s = ClState {
        sqrt_price,
        tick,
        liquidity,
        fee,
        ticks: map,
        version,
    };
    // Цена вниз (token0 → token1) — тики не выше текущего, по убыванию; вверх — выше текущего, по возрастанию.
    let next: Vec<i32> = if zfo {
        map.ticks
            .range(..=tick)
            .rev()
            .map(|(t, _)| *t)
            .take(n)
            .collect()
    } else {
        map.ticks
            .range(tick + 1..)
            .map(|(t, _)| *t)
            .take(n)
            .collect()
    };
    let mut out = Vec::new();
    for t in next {
        let Some(limit) = sqrt_ratio_at_tick(t) else {
            break;
        };
        if (zfo && limit >= sqrt_price) || (!zfo && limit <= sqrt_price) {
            continue;
        }
        let Ok((sw, at_limit)) = swap_cl::swap_to(&s, zfo, true, U256::from(u128::MAX), limit)
        else {
            break;
        };
        // Продажа — монета на входе; покупка — на выходе.
        let v = if sell { sw.amount_in } else { sw.amount_out };
        if !at_limit || v > max {
            break;
        }
        out.push(v);
    }
    out
}

/// Накопленная ёмкость бинов от активного по направлению обмена (в единицах монеты). Принимает: бины, активный id,
/// binStep, монета — X, продажа ли, наибольший объём, сколько изломов. Ошибок нет.
fn bins(
    b: &BinMap,
    active: u32,
    step: u32,
    x_coin: bool,
    sell: bool,
    max: U256,
    n: usize,
) -> Vec<U256> {
    let one = U256::from(1u8) << SCALE_OFFSET;
    // Выход X — бины от активного вверх; выход Y — от активного вниз (Liquidity Book: X выше активного, Y ниже).
    let x_out = (x_coin && !sell) || (!x_coin && sell);
    let ids: Vec<(u32, (u128, u128))> = if x_out {
        b.reserves.range(active..).map(|(i, r)| (*i, *r)).collect()
    } else {
        b.reserves
            .range(..=active)
            .rev()
            .map(|(i, r)| (*i, *r))
            .collect()
    };
    let mut cum = U256::ZERO;
    let mut out = Vec::new();
    for (id, (rx, ry)) in ids {
        let Some(price) = price_from_id(id, step) else {
            break;
        };
        // Сколько монеты проходит через бин: выход монеты — её резерв; вход монеты — резерв выхода по цене бина.
        let add = match (x_coin, sell) {
            (true, false) => Some(U256::from(rx)),
            (false, false) => Some(U256::from(ry)),
            (true, true) => mul_div(U256::from(ry), one, price),
            (false, true) => mul_div(U256::from(rx), price, one),
        };
        let Some(add) = add.filter(|a| !a.is_zero()) else {
            continue;
        };
        cum = cum.saturating_add(add);
        if cum > max || out.len() >= n {
            break;
        }
        out.push(cum);
    }
    out
}

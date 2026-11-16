//! Файл: venues/bsc_dex/quote/breaks_pool.rs
//! Что делает: решение 344 (задача t159, ревью t159 Н4) — изломы кривой одного пула по объёму монеты НА ЭТОМ ПУЛЕ
//! (минимальные единицы): объёмы, на которых у пула меняется кусок кривой обмена в нужном направлении. Это точки
//! подбора объёма, не данные решения; значения — той же математикой пула, что котировка (с комиссией). Виды:
//! - V3 (PancakeSwap / Uniswap V3, Slipstream) и Cl (Infinity CL, Uniswap V4): объём монеты до каждого следующего
//!   инициализированного тика по направлению обмена — обмен до предела цены тика (math/swap_cl.rs swap_to — та же
//!   математика и комиссия на вход, что котировка, pool_quote.rs); продажа — вход, покупка — выход;
//! - Bin (Infinity Bin) и LbBin (Liquidity Book V2.1) — quote/breaks_bins.rs (ёмкость бинов; при продаже — вход с
//!   комиссией);
//! - OraclePmm (WooFi, Metric, iZiSwap, Biswap V3): точки лесенки цен пула нужного направления (pool_quote.rs ветка
//!   OraclePmm, quote/pmm_ladder.rs) — на них средняя цена меняет отрезок интерполяции.
//!
//! Прочие виды (V2, Solidly, StableSwap, DMM, Balancer — гладкие кривые без изломов; Algebra, Elastic — нет общей
//! функции «обмен до тика»; DODO, SmarDex, Maverick — переходы не выведены, docs/notes/t159-…) — изломов нет (пусто).
//! Перевод в объём монеты всей стороны (доля сплита, налог, крючок) — quote/breaks.rs.
//! Кто вызывает: quote/breaks.rs.
//! От чего зависит: math/swap_cl.rs, math/tick_math.rs, quote/breaks_bins.rs, quote/pool_quote.rs (swap_fee),
//! pools/registry.rs (Entry), core/types (PoolKind, PoolState).
//! Настройки: нет (число изломов и наибольший объём даёт вызывающий).

use alloy_primitives::{Address, U256};

use crate::core::types::{PoolKind, PoolState};

use super::super::math::swap_cl::{self, ClState, Version};
use super::super::math::tick_math::sqrt_ratio_at_tick;
use super::super::pools::registry::Entry;
use super::breaks_bins::{BinSide, bins};
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
        (
            PoolState::Bin {
                active_id,
                protocol_fee,
                lp_fee,
            },
            PoolKind::Bin,
        ) => match (e.bins.as_ref(), e.pool.bin_step) {
            // Bin: X = currency0; комиссия обмена — по направлению (pool_quote.rs ветка Bin).
            (Some(b), Some(step)) => {
                let fee = BinSide::Inf(swap_fee(*protocol_fee, *lp_fee, zfo));
                bins(b, *active_id, step, coin0, sell, fee, max, n)
            }
            _ => Vec::new(),
        },
        (PoolState::LbBin(d), PoolKind::LbBin) => match e.bins.as_ref() {
            // X — token0 записи, если x_is_token0 (pool_quote.rs ветка LbBin).
            Some(b) => bins(
                b,
                d.active_id,
                d.bin_step,
                coin0 == d.x_is_token0,
                sell,
                BinSide::Lb(d),
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

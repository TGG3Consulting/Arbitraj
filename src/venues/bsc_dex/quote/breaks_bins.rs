//! Файл: venues/bsc_dex/quote/breaks_bins.rs
//! Что делает: решение 344 (задача t159, ревью t159 Н4) — изломы бинов одного пула по объёму монеты на пуле:
//! накопленная ёмкость бинов от активного по направлению обмена. Покупка (монета на выходе) — сумма резервов монеты
//! бинов. Продажа (монета на входе) — вход, нужный на выход всей ёмкости бинов до этого, С КОМИССИЕЙ на вход:
//! Infinity Bin — точным выходом той же математикой, что котировка (math/bin_swap.rs swap, exact_in = false);
//! Liquidity Book V2.1 — вход без комиссии по цене бина (math/bin_price.rs price_from_id, Y за 1 X · 2^128, вниз)
//! плюс НАИМЕНЬШАЯ комиссия обмена (math/lb_fee.rs: накопитель бина ≥ опорной волатильности references → total_fee
//! на ней, fee_on вверх): вход излома не больше нужного на выход этих бинов — точка не перескакивает свой бин
//! (tests/bsc_dex_breaks_t159.rs). Точка подбора, не данные решения.
//! Кто вызывает: quote/breaks_pool.rs.
//! От чего зависит: math/bin_price.rs, math/bin_swap.rs, math/full_math.rs, math/lb_fee.rs, pools/bins.rs,
//! core/types (LbBinData).
//! Настройки: нет.

use alloy_primitives::U256;

use crate::core::types::LbBinData;

use super::super::math::bin_price::{SCALE_OFFSET, price_from_id};
use super::super::math::full_math::mul_div;
use super::super::math::{bin_swap, lb_fee};
use super::super::pools::bins::BinMap;

/// Вид бинов и его комиссия.
pub enum BinSide<'a> {
    /// Infinity Bin: полная комиссия направления (пипсы, pool_quote.rs swap_fee).
    Inf(u32),
    /// Liquidity Book V2.1: данные пула (комиссия активного бина).
    Lb(&'a LbBinData),
}

/// Что делает: накопленная ёмкость бинов от активного по направлению обмена (в единицах монеты на пуле; при продаже —
/// вход с комиссией). Принимает: бины, активный id, binStep, монета — X, продажа ли, вид и комиссию, наибольший объём,
/// сколько изломов. Возвращает: объёмы по возрастанию. Ошибок нет: нет данных — пусто.
#[allow(clippy::too_many_arguments)]
pub fn bins(
    b: &BinMap,
    active: u32,
    step: u32,
    x_coin: bool,
    sell: bool,
    kind: BinSide<'_>,
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
    // LB: наименьшая комиссия обмена — на опорной волатильности (накопитель любого бина не меньше её; lb_fee.rs
    // accumulate) — нижняя граница входа; у Infinity Bin — точный выход.
    let lb_active = match &kind {
        BinSide::Lb(d) => {
            lb_fee::references(d, d.block_ts).and_then(|v| lb_fee::total_fee(d, v.reference))
        }
        BinSide::Inf(_) => None,
    };
    let (mut cum_in, mut cum_out) = (U256::ZERO, U256::ZERO);
    let mut out = Vec::new();
    for (id, (rx, ry)) in ids {
        if out.len() >= n {
            break;
        }
        let Some(price) = price_from_id(id, step) else {
            break;
        };
        let reserve_out = U256::from(if x_out { rx } else { ry });
        if reserve_out.is_zero() {
            continue;
        }
        cum_out = cum_out.saturating_add(reserve_out);
        let v = if !sell {
            // Покупка: монета на выходе — её резерв.
            Some(cum_out)
        } else {
            // Продажа: вход без комиссии по цене бина (X за Y: ·2^128 / price; Y за X: · price / 2^128).
            let add = if x_coin {
                mul_div(reserve_out, one, price)
            } else {
                mul_div(reserve_out, price, one)
            };
            let Some(add) = add else {
                break;
            };
            cum_in = cum_in.saturating_add(add);
            match &kind {
                BinSide::Inf(fee) => u128::try_from(cum_out)
                    .ok()
                    .and_then(|o| bin_swap::swap(b, active, *fee, x_coin, false, o))
                    .map(|s| s.amount_in),
                BinSide::Lb(_) => u128::try_from(cum_in).ok().and_then(|i| {
                    let fee = lb_active.and_then(|f| lb_fee::fee_on(i, f))?;
                    Some(U256::from(i.saturating_add(fee)))
                }),
            }
        };
        let Some(v) = v.filter(|v| !v.is_zero()) else {
            break;
        };
        if v > max {
            break;
        }
        out.push(v);
    }
    out
}

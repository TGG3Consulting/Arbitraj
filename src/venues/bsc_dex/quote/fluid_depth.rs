//! Файл: venues/bsc_dex/quote/fluid_depth.rs
//! Что делает: решение 440 — оценки по пулу Fluid DEX T1 (PoolState::SmartColDebt) для общих мест котировки:
//! - spot — цена для сизинга пробы (решение 73.19): отношение мнимых резервов основной части (смарт-залог, если
//!   включён, иначе смарт-долг) в единицах токенов;
//! - empty — пул пуст: пауза, не включена ни одна часть или у основной части нулевой мнимый резерв;
//! - fee_pct — комиссия пула в процентах (fee над 1e6);
//! - side — глубина в полосе цены dex.depth_band_pct (решение 73) без комиссии: по каждой включённой части x·y=k по
//!   мнимым резервам до сдвига цены на полосу, выход не дальше реального резерва (дошли — full = false); суммы частей.
//!
//! Это оценки для порога и размера пробы, а не сверяемые величины: котировку считает math/fluid_check.rs.
//! Кто вызывает: quote/{spot, emptiness, steps, depth_band}.rs.
//! От чего зависит: core/types (SmartColDebtData), math/full_math.rs, math/fluid_swap.rs (parts), num.rs.
//! Настройки: нет (полосу даёт вызывающий).
//!
//! Источник формул: Instadapp/fluid-contracts-public@404d73f poolT1/coreModule/helpers/coreHelpers.sol L76-L89
//! (_getAmountOut по мнимым резервам), L95-L105 (_getAmountIn), core/main.sol L95-L105 (комиссия над 1e6).

use alloy_primitives::{Address, U256};
use rust_decimal::Decimal;

use crate::core::types::SmartColDebtData;

use super::super::math::fluid_swap::{self, Part};
use super::super::math::full_math::mul_div;
use super::super::num;

/// Масштаб множителя полосы (18 знаков) — точность расчёта, не порог (как quote/depth_band.rs).
const SCALE: f64 = 1e18;

/// Основная часть пула по направлению: смарт-залог, если включён, иначе смарт-долг. Ошибок нет.
fn main_part(d: &SmartColDebtData, swap0to1: bool) -> Part {
    let (col, debt) = fluid_swap::parts(d, swap0to1);
    if d.col_on { col } else { debt }
}

/// Монет за сумму встречного токена по спот-цене основной части. Принимает: состояние, монету, сумму встречного
/// (единицы токена). Возвращает: монет (единицы токена) или None — нет цены / переполнение. Ошибок нет.
pub fn spot(d: &SmartColDebtData, coin: Address, amount: U256) -> Option<U256> {
    let coin0 = coin == d.token0;
    // Вход — встречный токен: swap0to1 = встречный — token0 пула.
    let p = main_part(d, !coin0);
    let (num_in, den_in, num_out, den_out) = if coin0 {
        (d.num1, d.den1, d.num0, d.den0)
    } else {
        (d.num0, d.den0, d.num1, d.den1)
    };
    let adj = mul_div(amount, num_in, den_in)?;
    if p.in_imag.is_zero() {
        return None;
    }
    let coin_adj = mul_div(adj, p.out_imag, p.in_imag)?;
    mul_div(coin_adj, den_out, num_out)
}

/// Пуст ли пул: пауза, ни одна часть не включена, у основной части нулевой мнимый резерв. Ошибок нет.
pub fn empty(d: &SmartColDebtData) -> bool {
    let p = main_part(d, true);
    d.paused || (!d.col_on && !d.debt_on) || p.in_imag.is_zero() || p.out_imag.is_zero()
}

/// Комиссия пула в процентах: fee над 1e6 → fee / 1e4. None — не помещается в Decimal. Ошибок нет.
pub fn fee_pct(d: &SmartColDebtData) -> Option<Decimal> {
    num::to_decimal(U256::from(d.fee), 4)
}

/// Одна часть в полосе: (вход, выход, дошли ли до края реального резерва) в приведённых единицах. None — переполнение.
fn part_band(p: &Part, grow: f64) -> Option<(U256, U256, bool)> {
    if p.in_imag.is_zero() || p.out_imag.is_zero() {
        return Some((U256::ZERO, U256::ZERO, false));
    }
    let f = U256::from((grow * SCALE) as u128);
    let eff = p.in_imag.checked_mul(f)? / U256::from(SCALE as u128);
    let out = mul_div(eff, p.out_imag, p.in_imag.checked_add(eff)?)?;
    if out < p.out_real {
        return Some((eff, out, false));
    }
    // Выход упёрся в реальный резерв: вход до него — _getAmountIn по мнимым резервам.
    let o = p.out_real.saturating_sub(U256::from(1u8));
    let rest = p.out_imag.checked_sub(o)?;
    let i = mul_div(o, p.in_imag, rest)?;
    Some((i, o, true))
}

/// Глубина одного направления в полосе (как depth_band::side): (сумма во встречном токене — покупка: отдано в пул,
/// продажа: получено; полоса прочитана целиком). Принимает: состояние, токен входа, полосу (доля), покупка ли.
/// Ошибки (текст): переполнение.
pub fn side(
    d: &SmartColDebtData,
    token_in: Address,
    b: f64,
    buy: bool,
) -> Result<(U256, bool), String> {
    let swap0to1 = token_in == d.token0;
    let grow = if buy {
        (1.0 + b).sqrt() - 1.0
    } else {
        1.0 / (1.0 - b).sqrt() - 1.0
    };
    let (col, debt) = fluid_swap::parts(d, swap0to1);
    let err = || "расчёт глубины пула Fluid невозможен".to_string();
    let (ci, co, cf) = part_band(&col, grow).ok_or_else(err)?;
    let (di, dout, df) = part_band(&debt, grow).ok_or_else(err)?;
    let (num_in, den_in, num_out, den_out) = if swap0to1 {
        (d.num0, d.den0, d.num1, d.den1)
    } else {
        (d.num1, d.den1, d.num0, d.den0)
    };
    let sum_in = ci.checked_add(di).ok_or_else(err)?;
    let sum_out = co.checked_add(dout).ok_or_else(err)?;
    let units = if buy {
        mul_div(sum_in, den_in, num_in)
    } else {
        mul_div(sum_out, den_out, num_out)
    };
    Ok((units.ok_or_else(err)?, !(cf || df)))
}

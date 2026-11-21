//! Файл: venues/bsc_dex/quote/basin_depth.rs
//! Что делает: решение 485 — оценки по скважине Basin (PoolState::BasinWell) для общих мест котировки:
//! - spot — цена для сизинга пробы (решение 73.19): ConstantProduct2 — отношение резервов (цена x·y = s² без
//!   комиссии); Stable2 — средняя цена обмена пробы (доля 1/PROBE_DIV резерва встречного токена, math/basin_well.rs);
//! - empty — скважина пуста: нулевой резерв или нулевое число LP-токенов;
//! - side — глубина в полосе цены dex.depth_band_pct (решение 73) по СРЕДНЕЙ цене исполнения, как у StableSwap
//!   (quote/stable_depth.rs): опорная цена — средняя цена пробы, удвоение объёма и двоичный поиск границы, на которой
//!   средняя цена уходит за полосу. Комиссии у скважины нет — глубина та же, что «без комиссии» (решение 73).
//!
//! Это оценки для порога и размера пробы, а не сверяемые величины: котировку считает math/basin_well.rs.
//! Кто вызывает: quote/{spot, emptiness, depth_band}.rs.
//! От чего зависит: core/types (BasinWellData), math/basin_well.rs, math/full_math.rs, quote/vault_depth.rs (PROBE_DIV).
//! Настройки: нет (полосу даёт вызывающий).

use alloy_primitives::{Address, U256};

use crate::core::types::{BasinFunction, BasinWellData};

use super::super::math::basin_well::swap_out;
use super::super::math::full_math::mul_div;
use super::vault_depth::PROBE_DIV;

/// Предел удвоений и шагов двоичного поиска (пределы итераций, не пороги торговли; как quote/stable_depth.rs).
const MAX_DOUBLE: u32 = 192;
const BISECT: u32 = 64;
/// Масштаб средней цены (18 знаков) — точность сравнения, не порог.
fn one() -> U256 {
    U256::from(10u8).pow(U256::from(18u8))
}

/// Другой токен скважины. Ошибок нет.
fn other(d: &BasinWellData, t: Address) -> Address {
    if t == d.token0 { d.token1 } else { d.token0 }
}

/// Монет за сумму встречного токена. Принимает: состояние, монету, сумму встречного. Возвращает: монет или None
/// (нулевой резерв, переполнение, проба не дала выхода). Ошибок нет.
pub fn spot(d: &BasinWellData, coin: Address, amount: U256) -> Option<U256> {
    let quote = other(d, coin);
    let (r_quote, r_coin) = d.sides(quote);
    if r_quote.is_zero() || r_coin.is_zero() {
        return None;
    }
    match d.function {
        BasinFunction::ConstantProduct2 => mul_div(amount, r_coin, r_quote),
        BasinFunction::Stable2 { .. } => {
            let probe = r_quote / U256::from(PROBE_DIV);
            let out = swap_out(d, quote, probe).filter(|o| !o.is_zero())?;
            mul_div(amount, out, probe)
        }
    }
}

/// Пуста ли скважина: резерв или число LP-токенов — ноль. Ошибок нет.
pub fn empty(d: &BasinWellData) -> bool {
    d.reserve0.is_zero() || d.reserve1.is_zero() || d.supply.is_zero()
}

/// Средняя цена обмена `v` (выход · 1e18 / вход). None — нет выхода. Ошибок нет.
fn avg(d: &BasinWellData, token_in: Address, v: U256) -> Option<U256> {
    let out = swap_out(d, token_in, v).filter(|o| !o.is_zero())?;
    mul_div(out, one(), v)
}

/// Глубина одного направления в полосе (как depth_band::side): (сумма во встречном токене — покупка: отдано в
/// скважину, продажа: получено; граница найдена). Принимает: состояние, токен входа, полосу (доля), покупка ли.
/// Ошибки (текст): полоса вне 0–100%, пустая скважина, расчёт невозможен.
pub fn side(
    d: &BasinWellData,
    token_in: Address,
    b: f64,
    buy: bool,
) -> Result<(U256, bool), String> {
    if !(b > 0.0 && b < 1.0) {
        return Err("полоса вне 0–100%".into());
    }
    let (r_in, _) = d.sides(token_in);
    let probe = r_in / U256::from(PROBE_DIV);
    let err = || "расчёт глубины скважины Basin невозможен".to_string();
    if probe.is_zero() {
        return Ok((U256::ZERO, true));
    }
    let p0 = avg(d, token_in, probe).ok_or_else(err)?;
    // Средняя цена «выход за вход» падает с объёмом: за полосу b — ниже p0·(1 − b). Покупка (вход — встречный) и
    // продажа (вход — монета) — одна и та же граница на средней цене выхода к входу.
    let bp = (b * 10_000.0) as u64;
    let limit = mul_div(p0, U256::from(10_000 - bp), U256::from(10_000u64)).ok_or_else(err)?;
    let inside = |v: U256| avg(d, token_in, v).is_some_and(|p| p >= limit);
    let (mut lo, mut hi, mut found) = (probe, probe, false);
    for _ in 0..MAX_DOUBLE {
        hi = hi.checked_mul(U256::from(2u8)).ok_or_else(err)?;
        if !inside(hi) {
            found = true;
            break;
        }
        lo = hi;
    }
    if found {
        for _ in 0..BISECT {
            let mid = lo + (hi - lo) / U256::from(2u8);
            if mid == lo {
                break;
            }
            if inside(mid) { lo = mid } else { hi = mid }
        }
    }
    let amount = if buy {
        lo
    } else {
        swap_out(d, token_in, lo).ok_or_else(err)?
    };
    Ok((amount, found))
}

//! Файл: venues/bsc_dex/quote/tick_bin_spot.rs
//! Что делает: цена и глубина пула TickBin (Maverick V2, решение 122 F) для размера пробы (решение 73.19) и глубины в
//! полосе (решение 73). Цена — sqrt-цена активного тика по его резервам (официальный TickMath getTickSqrtPriceAndL,
//! math/maverick_tick_math.rs): sqrtPrice² — цена tokenB в tokenA в D18-масштабе пула (getSqrtPrice: √p = (A + L·√pl)
//! / (B + L/√pu) — v2-common d91e871 TickMath.sol; вход tokenA поднимает тик — maverick-v2-examples README L145).
//! Глубина — порт расчёта обмена (math/maverick_swap.rs) БЕЗ комиссии (решение 73) до предела тика: активный ± тиков
//! полосы (ширина тика — 1.0001^tickSpacing, IMaverickV2Pool.sol L214-L217).
//! Кто вызывает: quote/spot.rs, quote/depth_band.rs.
//! От чего зависит: math/maverick_swap.rs, math/maverick_tick_math.rs, math/full_math.rs, core/types (TickBinData).
//! Настройки: нет (полосу даёт вызывающий).

use alloy_primitives::U256;

use crate::core::types::TickBinData;

use super::super::math::full_math::mul_div;
use super::super::math::maverick_swap;
use super::super::math::maverick_tick_math::{get_tick_sqrt_price_and_l, tick_sqrt_prices};

/// sqrt-цена активного тика (D18). None — тика нет среди прочитанных (пуст) или откат TickMath. Ошибок нет.
fn active_sqrt_price(d: &TickBinData) -> Option<U256> {
    let t = d.ticks.iter().find(|t| t.tick == d.active_tick)?;
    let (lo, hi) = tick_sqrt_prices(d.tick_spacing, d.active_tick)?;
    let (sp, _) =
        get_tick_sqrt_price_and_l(U256::from(t.reserve0), U256::from(t.reserve1), lo, hi)?;
    (!sp.is_zero()).then_some(sp)
}

/// Монеты на сумму встречного токена по цене активного тика. Принимает: состояние, монета — token0 (tokenA), сумму.
/// None — цены нет, переполнение. Ошибок нет.
pub fn coins_for(d: &TickBinData, coin0: bool, amount: U256) -> Option<U256> {
    let sp = active_sqrt_price(d)?;
    let e18 = U256::from(10u8).pow(U256::from(18u8));
    if coin0 {
        // Монета A, встречный B: A = B·p, p = sp²/1e36 (обе стороны в D18 пула).
        let b = amount.checked_mul(d.scale1)?;
        let a = mul_div(mul_div(b, sp, e18)?, sp, e18)?;
        Some(a / d.scale0)
    } else {
        let a = amount.checked_mul(d.scale0)?;
        let b = mul_div(mul_div(a, e18, sp)?, e18, sp)?;
        Some(b / d.scale1)
    }
}

/// Глубина одной стороны без комиссии до границы полосы. Принимает: состояние, вход tokenA, тиков полосы (в единицах
/// 1.0001), большую сумму входа. Возвращает: (вход, выход, дошли до границы — не упёрлись в непрочитанный тик).
/// None — расчёт невозможен. Ошибок нет.
pub fn depth(
    d: &TickBinData,
    a_in: bool,
    band_ticks: i32,
    amount: U256,
) -> Option<(U256, U256, bool)> {
    let ts = i32::try_from(d.tick_spacing).ok().filter(|t| *t > 0)?;
    let n = band_ticks / ts;
    let limit = if a_in {
        d.active_tick.checked_add(n)?
    } else {
        d.active_tick.checked_sub(n)?
    };
    let mut free = d.clone();
    free.fee0_in = U256::ZERO;
    free.fee1_in = U256::ZERO;
    let r = maverick_swap::swap(&free, amount, a_in, false, limit)?;
    Some((r.amount_in, r.amount_out, !r.unread))
}

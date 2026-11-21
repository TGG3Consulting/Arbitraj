//! Файл: venues/bsc_dex/quote/depth_band_algebra.rs
//! Что делает: решение 205 — глубина пула Algebra (THENA Fusion — Algebra V1.9, Algebra Integral — решение 188) в полосе
//! цены dex.depth_band_pct: обмен до цены на границе полосы той же математикой, что котировка (math/swap_algebra.rs
//! swap_to), без комиссии пула (решение Тиграна к решению 73 — чистый объём). Раньше состояние Algebra (вид ядра Cl)
//! в quote/depth_band.rs не разбиралось: «вид пула и состояние не совпадают» — сторона пула исключалась из котировок
//! (журнал 06.11, thena-fusion).
//! Кто вызывает: quote/depth_band.rs (side).
//! От чего зависит: math/swap_algebra.rs, math/tick_math.rs, quote/depth_band.rs (band_ticks).
//! Настройки: нет (полосу даёт вызывающий).

use alloy_primitives::U256;

use super::super::math::swap_algebra::{self, AlgebraState};
use super::super::math::tick_math::{MAX_TICK, MIN_TICK, sqrt_ratio_at_tick};
use super::depth_band::band_ticks;

/// Обмен до цены на границе полосы. Принимает: состояние (комиссия — 0), направление (true — token0 → token1),
/// полосу (доля), покупка ли (вверх). Возвращает: (отдано, получено, дошли до границы). Ошибки (текст): расчёт.
pub fn algebra(
    s: &AlgebraState,
    zfo: bool,
    b: f64,
    buy: bool,
) -> Result<(U256, U256, bool), String> {
    // Граница — от текущего тика, как у V3 (quote/depth_band.rs cl): цена тика 1,0001^tick у Algebra та же (TickMath).
    let d = band_ticks(b, buy).ok_or("полоса вне 0–100%")?;
    if d == 0 {
        return Ok((U256::ZERO, U256::ZERO, true));
    }
    let edge = if zfo { s.tick - d } else { s.tick + d }.clamp(MIN_TICK + 1, MAX_TICK - 1);
    let limit = sqrt_ratio_at_tick(edge).ok_or("цена границы полосы вне диапазона TickMath")?;
    let (r, reached) = swap_algebra::swap_to(s, zfo, true, U256::from(u128::MAX), limit)
        .map_err(str::to_string)?;
    Ok((r.amount_in, r.amount_out, reached))
}

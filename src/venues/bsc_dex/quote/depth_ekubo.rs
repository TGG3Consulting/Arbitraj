//! Файл: venues/bsc_dex/quote/depth_ekubo.rs
//! Что делает: глубина пула Ekubo по одному направлению в полосе цены (решение 447, к решению 73): обмен без комиссии
//! пула (и без доплаты MEVCapture — глубина для порога, комиссии — в котировке) до цены на границе полосы. Граница —
//! от текущего тика на Δ тиков Ekubo: цена тика = 1,000001^tick (ticks.sol L12), вверх Δ = ln(1+b)/ln(1,000001), вниз —
//! −ln(1−b)/ln(1,000001), вниз до целого. Полоса не прочитана целиком (кончились слова битмапа) — глубина до края
//! прочитанного и отметка «не целиком».
//! Кто вызывает: quote/depth_band.rs (ветка Ekubo).
//! От чего зависит: core/types (EkuboData), math/ekubo_swap.rs, math/ekubo_tick.rs.
//! Настройки: нет (полосу даёт вызывающий — dex.depth_band_pct).
//!
//! Источник — EkuboProtocol/evm-contracts@61d09daa src/math/ticks.sol L12 («sqrtRatio = sqrt(1.000001^tick)»),
//! src/types/poolConfig.sol L32-L36 (комиссия — биты 32..95 config: обнуляются в копии состояния).

use alloy_primitives::U256;

use crate::core::types::EkuboData;

use super::super::math::ekubo_swap;
use super::super::math::ekubo_tick::{MAX_TICK, MIN_TICK, to_sqrt_ratio_at};

/// Основание цены тика Ekubo: 1,000001 (ticks.sol L12) — свойство протокола, не настройка.
const TICK_BASE: f64 = 1.000001;

/// Одно направление. Принимает: состояние, вход — token0 (zfo), полосу (доля 0..1), покупка ли (иначе продажа).
/// Возвращает: (покупка — отдано, продажа — получено; полоса прочитана целиком). Ошибки (текст): расчёт невозможен.
pub fn side(d: &EkuboData, zfo: bool, b: f64, buy: bool) -> Result<(U256, bool), String> {
    let ticks = if buy {
        (1.0 + b).ln() / TICK_BASE.ln()
    } else {
        -(1.0 - b).ln() / TICK_BASE.ln()
    };
    if !ticks.is_finite() || ticks < 0.0 {
        return Err("полоса вне 0–100%".into());
    }
    let dt = ticks.floor().min(f64::from(MAX_TICK)) as i32;
    if dt == 0 {
        return Ok((U256::ZERO, true));
    }
    let edge = if zfo {
        d.tick.saturating_sub(dt)
    } else {
        d.tick.saturating_add(dt)
    }
    .clamp(MIN_TICK + 1, MAX_TICK - 1);
    let limit = to_sqrt_ratio_at(edge).ok_or("цена границы полосы вне диапазона тиков Ekubo")?;
    let mut nf = d.clone();
    nf.config.0[20..28].fill(0);
    let all = i128::MAX as u128;
    let r = ekubo_swap::exact_in_to(&nf, !zfo, all, limit)
        .ok_or("расчёт обмена Ekubo до границы полосы невозможен")?;
    let amount = if buy { r.amount_in } else { r.amount_out };
    Ok((U256::from(amount), r.at_limit))
}

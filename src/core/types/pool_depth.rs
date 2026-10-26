//! Файл: core/types/pool_depth.rs
//! Что делает: тип «что адаптер DEX знает о пуле сверх состояния» для экрана «Монеты» (решение 73): глубина по
//! направлениям в полосе цены и вердикт (с гистерезисом), слушается ли пул полностью, итог независимой сверки расчёта
//! пула; подробности, которые есть только у площадки, — строками описания (`details`), без полей под конкретную DEX.
//! Кто вызывает: адаптер DEX (заполняет — Venue::pool_depths), api/coins.rs (кладёт в пулы GET /api/coins), интерфейс.
//! От чего зависит: core/types/market.rs, core/types/pool.rs, rust_decimal.
//! Настройки: нет.
//!
//! Это стык: менять только через запрос на изменение стыка (docs/AGENTS.md).

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use super::market::VenueId;
use super::pool::PoolRef;

/// Итог независимой сверки расчёта пула площадкой. В JSON — «not_checked» | «ok» | «failed».
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CrossCheck {
    /// Ещё не сверен.
    NotChecked,
    /// Сошлось.
    Ok,
    /// Расхождение — причина в cross_check_reason.
    Failed,
}

/// Итог сверки одной стороны.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SideCheck {
    pub state: CrossCheck,
    /// Причина расхождения (state = failed).
    pub reason: Option<String>,
}

/// Пул DEX глазами адаптера (см. шапку).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PoolDepth {
    pub venue: VenueId,
    pub pool: PoolRef,
    /// Покупка монеты: сколько USDT пул примет до цены +полоса. None — не посчитана (причина — depth_error).
    pub buy_usd: Option<Decimal>,
    /// Продажа монеты: сколько USDT пул отдаст до цены −полоса.
    pub sell_usd: Option<Decimal>,
    /// Полоса, % (dex.depth_band_pct на момент расчёта).
    pub band_pct: Option<Decimal>,
    /// Порог dex.min_pool_liquidity_usd на момент расчёта.
    pub min_usd: Option<Decimal>,
    /// Полоса прочитана целиком (иначе глубина — до края прочитанной карты тиков / бинов).
    pub full: bool,
    /// Направление глубокое (с гистерезисом).
    pub deep_buy: bool,
    pub deep_sell: bool,
    /// Глубину не посчитать — почему.
    pub depth_error: Option<String>,
    /// Пул «спит»: площадка слушает у него только рост ликвидности (мелкий по обоим направлениям).
    pub asleep: bool,
    /// Данные пула достоверны.
    pub valid: bool,
    /// Независимая сверка расчёта пула площадкой (у DEX — квотер) по каждой стороне: покупка и продажа сверяются
    /// независимо, сторона годна для сделки только со своей прошедшей сверкой (решение 73).
    pub cross_check_buy: SideCheck,
    pub cross_check_sell: SideCheck,
    /// Подробности площадки строками (например «tickSpacing 1») — ядро их не разбирает, только показывает.
    pub details: Vec<String>,
}

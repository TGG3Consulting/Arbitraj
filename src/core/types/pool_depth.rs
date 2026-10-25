//! Файл: core/types/pool_depth.rs
//! Что делает: тип «что адаптер DEX знает о пуле сверх состояния» для экрана «Монеты» (решение 73): глубина по
//! направлениям в полосе цены и вердикт (с гистерезисом), спит ли пул, итог сверки с квотером, tickSpacing.
//! Кто вызывает: адаптер DEX (заполняет — Venue::pool_depths), api/coins.rs (кладёт в пулы GET /api/coins), интерфейс.
//! От чего зависит: core/types/market.rs, core/types/pool.rs, rust_decimal.
//! Настройки: нет.
//!
//! Это стык: менять только через запрос на изменение стыка (docs/AGENTS.md).

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use super::market::VenueId;
use super::pool::PoolRef;

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
    /// Пул спит: слушается только добавление ликвидности (источник ws, мелкий по обеим сторонам).
    pub asleep: bool,
    /// Данные пула достоверны.
    pub valid: bool,
    /// Сверка с квотером: «not_checked» | «ok» | «failed».
    pub quoter: String,
    /// Причина расхождения с квотером (quoter = failed).
    pub quoter_reason: Option<String>,
    /// tickSpacing (V3, CL); иначе None.
    pub tick_spacing: Option<i32>,
}

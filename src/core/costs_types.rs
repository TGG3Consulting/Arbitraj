//! Файл: core/costs_types.rs
//! Что делает: типы полного расчёта направления (core/costs.rs): строка затрат и расчёт на объём — с тем, как
//! подобран объём (решение 66). Вынесены из costs.rs, чтобы файл расчёта оставался коротким.
//! Кто вызывает: core/costs.rs (собирает), profit.rs, candidates.rs, executor.rs, partial.rs, интерфейс (JSON).
//! От чего зависит: core/types, core/size.rs (SizeChoice).
//! Настройки: нет.

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::Serialize;

use crate::core::types::{CostKind, Direction, Quote, RouteStep};

use super::size::SizeChoice;

/// Одна строка затрат расчёта.
#[derive(Debug, Clone, Serialize)]
pub struct CostLine {
    pub kind: CostKind,
    /// Где возникла: «A» (покупка), «перевод», «B» (продажа), «возврат USDT», «ядро» (запас r).
    pub stage: &'static str,
    pub amount: Decimal,
    pub asset: String,
    /// В USDT по живым ценам. None — только у затраты «в цене» без живой цены её валюты (показ;
    /// в итог она не входит — уже в средней цене котировки).
    pub usdt: Option<Decimal>,
    /// Уже в средней цене котировки — повторно не вычитается.
    pub in_price: bool,
    /// Затрата в монете: уменьшает количество (до продажи доходит меньше) — повторно не вычитается.
    pub in_qty: bool,
    /// Шаг маршрута обмена (решение 52): «USDT → BNB», «BNB → монета»; None — не шаг обмена.
    pub step: Option<RouteStep>,
}

/// Расчёт направления на объём.
#[derive(Debug, Clone, Serialize)]
pub struct Calc {
    pub direction: Direction,
    /// Сколько покупаем на A.
    pub qty: Decimal,
    /// Сколько дойдёт до B.
    pub arrive: Decimal,
    /// Сколько продаём на B: приход вниз до шага пары биржи B (у DEX — весь приход).
    pub sell_qty: Decimal,
    pub buy_avg: Decimal,
    pub sell_avg: Decimal,
    /// Вложено: qty × buy_avg.
    pub invested: Decimal,
    /// Выручка: arrive × sell_avg.
    pub proceeds: Decimal,
    pub costs: Vec<CostLine>,
    /// Затраты, вычитаемые из прибыли (не в цене и не в количестве), USDT.
    pub costs_usdt: Decimal,
    pub profit_usd: Decimal,
    pub profit_pct: Decimal,
    pub transit_s: Decimal,
    pub sigma: Decimal,
    /// Котировки — для худшей цены ордеров и маршрута DEX при исполнении.
    pub buy_quote: Quote,
    pub sell_quote: Quote,
    pub at: DateTime<Utc>,
    /// Как подобран объём (решение 66): пределы, прибыль на бюджете, почему этот лучший. None — расчёт на заданный
    /// объём без подбора (partial.rs).
    pub size: Option<SizeChoice>,
}

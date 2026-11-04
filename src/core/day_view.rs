//! Файл: core/day_view.rs
//! Что делает: итог дня для плашки режима и экрана «Управление» (решения 53, 152, 153): дата, всего, фактическая
//! часть и оценка позиций, неразобранные оценки сделок (и какие из них без цены — худший случай), сброс лимита дня
//! и состояние лимита (сколько из скольких, превышен ли). Значения ведёт core/daily.rs; лимит — core/daily_limit.rs.
//! Кто вызывает: core/risk.rs (поле Risk.day), core/daily.rs, core/daily_limit.rs.
//! От чего зависит: chrono, rust_decimal.
//! Настройки: нет (лимит — risk.max_daily_loss_usd, читает daily_limit.rs).

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, NaiveDate, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// Отметка «Сбросить лимит дня» (решение 153): с этого момента лимит считает только изменения итога дня после неё.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LimitReset {
    /// Время нажатия.
    pub at: DateTime<Utc>,
    /// Итог дня (факт + оценки сегодняшних позиций) в момент сброса, USDT.
    pub base_usd: Decimal,
    /// Кто сбросил (интерфейс).
    pub by: String,
}

/// Состояние лимита дня (risk.max_daily_loss_usd) — считается при каждом чтении (daily_limit.rs).
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct LimitView {
    /// Лимит, USDT.
    pub max_usd: Decimal,
    /// Убыток, который считает лимит: −(итог дня − итог в момент сброса); без сброса — −итог дня. Не меньше 0.
    pub loss_usd: Decimal,
    /// Лимит достигнут (loss_usd ≥ max_usd).
    pub exceeded: bool,
}

/// Итог дня для экрана «Управление».
#[derive(Debug, Clone, Serialize)]
pub struct DayView {
    pub date: NaiveDate,
    /// Всего за день: факт + оценки позиций (остановленных и «Беру на себя»), учтённых сегодня.
    pub result_usd: Decimal,
    /// Фактическая часть (закрытые сделки и коррекции разборов в этот день).
    pub final_usd: Decimal,
    /// Оценка позиций, учтённых сегодня (решение 152: стоимость монет по рынку − вложено; без цены — худший случай).
    pub provisional_usd: Decimal,
    /// Все неразобранные оценки: сделка → (сумма, день учёта).
    pub provisional: BTreeMap<String, (Decimal, NaiveDate)>,
    /// Сделки, у которых цены не было — в оценке худший случай (вся вложенная сумма).
    pub unpriced: BTreeSet<String>,
    /// Сброс лимита дня сегодня (решение 153); None — не сбрасывался.
    pub limit_reset: Option<LimitReset>,
    /// Лимит дня; None — risk.max_daily_loss_usd не задана.
    pub limit: Option<LimitView>,
}

impl Default for DayView {
    fn default() -> Self {
        DayView {
            date: Utc::now().date_naive(),
            result_usd: Decimal::ZERO,
            final_usd: Decimal::ZERO,
            provisional_usd: Decimal::ZERO,
            provisional: Default::default(),
            unpriced: Default::default(),
            limit_reset: None,
            limit: None,
        }
    }
}

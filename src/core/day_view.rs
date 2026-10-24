//! Файл: core/day_view.rs
//! Что делает: итог дня для экрана «Управление» (решение 53): дата, всего, фактическая и предварительная
//! части, неразобранные предварительные убытки сделок. Значения ведёт core/daily.rs.
//! Кто вызывает: core/risk.rs (поле Risk.day), core/daily.rs.
//! От чего зависит: chrono, rust_decimal.
//! Настройки: нет.

use chrono::{NaiveDate, Utc};
use rust_decimal::Decimal;
use serde::Serialize;

/// Итог дня для экрана «Управление» (решение 53).
#[derive(Debug, Clone, Serialize)]
pub struct DayView {
    pub date: NaiveDate,
    /// Всего за день: фактический + предварительный сегодняшних остановленных и «Беру на себя».
    pub result_usd: Decimal,
    /// Фактическая часть (закрытые сделки и коррекции разборов в этот день).
    pub final_usd: Decimal,
    /// Предварительная часть: убыток по худшему варианту сделок, учтённых сегодня.
    pub provisional_usd: Decimal,
    /// Все неразобранные предварительные убытки: сделка → (сумма, день учёта).
    pub provisional: std::collections::BTreeMap<String, (Decimal, NaiveDate)>,
}

impl Default for DayView {
    fn default() -> Self {
        DayView {
            date: Utc::now().date_naive(),
            result_usd: Decimal::ZERO,
            final_usd: Decimal::ZERO,
            provisional_usd: Decimal::ZERO,
            provisional: Default::default(),
        }
    }
}

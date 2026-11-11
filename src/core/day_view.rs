//! Файл: core/day_view.rs
//! Что делает: итог дня для плашки режима и экрана «Управление» (решения 53, 152, 153, 173): дата, всего, фактическая
//! часть и оценка позиций, неразобранные оценки сделок (и какие из них без цены — худший случай), сброс лимита дня
//! и состояние лимита (сколько из скольких, превышен ли) — всей системы и по каждой площадке (решение 173: у площадки
//! свой итог, свой лимит, своя пауза). Значения ведёт core/daily.rs; лимиты — core/daily_limit.rs, daily_venue.rs.
//! Кто вызывает: core/risk.rs (поле Risk.day), core/daily.rs, core/daily_limit.rs, core/daily_venue.rs.
//! От чего зависит: chrono, rust_decimal.
//! Настройки: нет (лимиты — risk.max_daily_loss_usd по площадке и risk.max_daily_loss_total_usd; читают
//! daily_venue.rs и daily_limit.rs).

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

/// Состояние лимита дня — считается при каждом пересчёте: всей системы (risk.max_daily_loss_total_usd,
/// daily_limit.rs) или площадки (risk.max_daily_loss_usd, daily_venue_limit.rs).
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct LimitView {
    /// Лимит, USDT.
    pub max_usd: Decimal,
    /// Убыток, который считает лимит: −(итог дня − итог в момент сброса); без сброса — −итог дня. Не меньше 0.
    pub loss_usd: Decimal,
    /// Лимит достигнут (loss_usd ≥ max_usd).
    pub exceeded: bool,
}

impl LimitView {
    /// Что делает: состояние лимита: убыток = −(итог − итог в момент сброса этого дня); без сброса — −итог.
    /// Принимает: лимит, отметку сброса (действует только в день `date`), день, итог. Ошибок нет.
    pub fn of(
        max: Decimal,
        reset: Option<&LimitReset>,
        date: NaiveDate,
        result: Decimal,
    ) -> LimitView {
        let base = reset
            .filter(|r| r.at.date_naive() == date)
            .map_or(Decimal::ZERO, |r| r.base_usd);
        let loss = base - result;
        LimitView {
            max_usd: max,
            loss_usd: loss.max(Decimal::ZERO),
            exceeded: loss >= max,
        }
    }
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
    /// Сброс общего лимита дня сегодня (решение 153); None — не сбрасывался.
    pub limit_reset: Option<LimitReset>,
    /// Общий лимит дня всей системы (решение 173: risk.max_daily_loss_total_usd — «Стоп» всей системы);
    /// None — не задан, общего стопа по убытку нет.
    pub limit: Option<LimitView>,
    /// Решение 173: итог дня по площадкам (ключ — id площадки). Сделка CEX ↔ DEX — у биржи, CEX ↔ CEX — у биржи
    /// покупки (дополнение t91), DEX ↔ DEX — у DEX (daily_venue::owner). Сумма по площадкам = общий итог дня.
    pub venues: BTreeMap<String, VenueDay>,
    /// Решение 173: площадки неразобранных оценок (сделка → id площадок).
    pub deal_venues: BTreeMap<String, Vec<String>>,
}

/// Итог дня одной площадки (решение 173) и её лимит.
#[derive(Debug, Clone, Serialize, Default, PartialEq)]
pub struct VenueDay {
    /// Всего: факт + оценки позиций её сделок, учтённых сегодня.
    pub result_usd: Decimal,
    /// Факт закрытых сегодня сделок с её участием.
    pub final_usd: Decimal,
    /// Оценка позиций её сделок, учтённых сегодня.
    pub provisional_usd: Decimal,
    /// Сброс лимита дня этой площадки сегодня; None — не сбрасывался.
    pub limit_reset: Option<LimitReset>,
    /// Лимит площадки (risk.max_daily_loss_usd: значение площадки, иначе «вся система»); None — не задан.
    pub limit: Option<LimitView>,
    /// Площадка на паузе по убытку дня (Risk.venue_paused с причиной лимита).
    pub paused: bool,
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
            venues: BTreeMap::new(),
            deal_venues: BTreeMap::new(),
        }
    }
}

//! Файл: core/signal_types.rs
//! Что делает: типы сигнала (шаг 3 ТЗ) — цены направления, состояние направления и строка «Живого экрана».
//! Вынесены из core/signal.rs (ориентир 200 строк); signal.rs их реэкспортирует — пути signal::Prices и т. д. прежние.
//! Кто вызывает: core/signal.rs, core/evaluate.rs, core/prices.rs и все, кто читает строки «Живого экрана».
//! От чего зависит: core/types (Direction), core/signal_fast.rs (Trigger, решение 317).
//! Настройки: нет.

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::Serialize;

use super::signal_fast::Trigger;
use crate::core::types::Direction;

/// Цены направления на объём сравнения (prices.rs).
#[derive(Debug, Clone, Serialize)]
pub struct Prices {
    /// Средняя цена покупки на A.
    pub buy: Decimal,
    /// Средняя цена продажи на B.
    pub sell: Decimal,
    /// Сумма процентных комиссий обеих сторон, которых нет в цене (торговая комиссия биржи), %.
    pub fees_pct: Decimal,
    /// Объём сравнения, монет: меньшая из глубин на потолке (решение 87) или, если задан signal.probe_usd, на объёме
    /// проверки сигнала (решение 260, core/signal_probe.rs).
    pub qty: Decimal,
}

/// Состояние направления на «Живом экране».
#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum State {
    /// Нет цены на одной из сторон — причина как есть.
    NoPrice(String),
    /// Не заданы пороги фильтра шума.
    NotConfigured(String),
    /// Спред не покрывает комиссии.
    Below,
    /// Спред выше порога, но ещё не продержался нужное время / число обновлений.
    Holding,
    /// Спред аномально велик — нужна перепроверка статусов ввода и вывода.
    Anomaly,
    /// Пауза направления после сделки или ошибки.
    Cooldown { until: DateTime<Utc> },
    /// Кандидат: прошёл порог и все фильтры — уходит в полный расчёт.
    Candidate,
    /// Налоги монеты ещё не проверены (решение 30): строка видна, торговли нет — причина ожидания.
    TaxesNotChecked(String),
}

/// Строка «Живого экрана».
#[derive(Debug, Clone, Serialize)]
pub struct SpreadView {
    pub direction: Direction,
    pub prices: Option<Prices>,
    /// Грубый спред, %.
    pub spread_pct: Option<Decimal>,
    pub state: State,
    /// С какого момента спред держится выше порога.
    pub above_since: Option<DateTime<Utc>>,
    /// Сколько обновлений подряд выше порога.
    pub updates: u64,
    pub at: DateTime<Utc>,
    /// Что запустило пересчёт (решения 317, 318).
    pub trigger: Trigger,
    /// Решение 317: кандидат выдан без удержания (событие пула, стакан биржи стоит).
    pub skipped_hold: bool,
    /// Решение 317: сколько мс не менялись уровни стакана биржи на объёме сравнения (только биржа ↔ DEX; отметка из
    /// состояния рынка ядра, book_still.rs).
    pub cex_still_ms: Option<u64>,
    /// Решение 318: срок дозревания «держится» — таймер сам поставит пересчёт (только при signal.hold_timer = да).
    pub ripen_at: Option<DateTime<Utc>>,
}

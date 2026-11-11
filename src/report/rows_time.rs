//! Файл: report/rows_time.rs
//! Что делает: решение 257 — время конца сделки и окно выгрузки отчёта «Итоги сделок».
//! Конец сделки — запись хронологии, которую ядро делает в момент перехода в конечный шаг (факт кода, а не updated_at:
//! updated_at двигает любая запись хронологии, и после закрытия — возврат USDT, газ перегона, уведомление):
//! - core/ledger.rs finish: «Сделка закрыта: …» / «Сделка остановлена: …» / «Сделка монеты у человека: …»
//!   (ledger::end_prefix текущего шага); берётся последняя такая запись;
//! - core/ledger_load.rs: «Сделка прервана перезапуском …» (ledger_load::INTERRUPTED) — остановка при запуске (Halted).
//!   Нет такой записи — конец неизвестен (в отчёте пусто, день — по старту, с пометкой).
//!
//! Окно: [from, to) по UTC; to нет — до момента выгрузки. Кнопки «Сутки / 7 дней / 30 дней» (Telegram) — календарные дни
//! UTC: с 00:00 UTC дня (сегодня − (N − 1)) до момента выгрузки (calendar_from).
//! Кто вызывает: report/rows.rs, report/day_rows.rs, report/results_xlsx.rs, notify/ctl/report.rs.
//! От чего зависит: core/deal.rs, core/ledger.rs (end_prefix), core/ledger_load.rs (INTERRUPTED).
//! Настройки: нет.

use chrono::{DateTime, Duration, NaiveDate, Utc};

use crate::core::deal::{Deal, Step};

/// Окно выгрузки: [from, to); None — без границы (to None — до момента выгрузки `now`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Window {
    pub from: Option<DateTime<Utc>>,
    pub to: Option<DateTime<Utc>>,
    pub now: DateTime<Utc>,
}

impl Window {
    /// Время в окне. Ошибок нет.
    pub fn has(&self, t: DateTime<Utc>) -> bool {
        self.from.is_none_or(|f| t >= f) && self.to.is_none_or(|x| t < x)
    }

    /// День UTC в окне целиком: от 00:00 до 24:00 (или до момента выгрузки, если это сегодня и окно до выгрузки).
    /// Ошибок нет.
    pub fn whole_day(&self, d: NaiveDate) -> bool {
        let start = d.and_hms_opt(0, 0, 0).map(|t| t.and_utc());
        let Some(start) = start else { return false };
        let end = start + Duration::days(1);
        let from_ok = self.from.is_none_or(|f| f <= start);
        let to_ok = match self.to {
            Some(t) => end <= t,
            None => end <= self.now || d == self.now.date_naive(),
        };
        from_ok && to_ok
    }

    /// Окно текстом для ячейки «Окно выгрузки»: дата и время UTC. Ошибок нет.
    pub fn text(&self) -> String {
        let f = |t: DateTime<Utc>| t.format("%Y-%m-%d %H:%M:%S").to_string();
        let a = self.from.map_or(
            "с первой сделки в учёте".to_string(),
            |t| format!("с {} UTC", f(t)),
        );
        let b = self.to.map_or(
            format!("по {} UTC (момент выгрузки)", f(self.now)),
            |t| format!("по {} UTC (не включая)", f(t)),
        );
        format!("{a} {b}")
    }
}

/// Начало окна кнопки «N дней» календарными днями UTC: 00:00 UTC дня (сегодня − (N − 1)). N = 1 — сегодня с 00:00 UTC.
/// Принимает: число дней (≥ 1), момент выгрузки. Ошибок нет.
pub fn calendar_from(days: i64, now: DateTime<Utc>) -> DateTime<Utc> {
    let d = now.date_naive() - Duration::days(days.max(1) - 1);
    d.and_hms_opt(0, 0, 0).map_or(now, |t| t.and_utc())
}

/// Время конца сделки по хронологии (см. шапку). Открытая сделка — None. Ошибок нет.
pub fn ended_at(d: &Deal) -> Option<DateTime<Utc>> {
    if d.is_open() {
        return None;
    }
    let own = crate::core::ledger::end_prefix(d.step);
    let restart = crate::core::ledger_load::INTERRUPTED;
    d.timeline
        .iter()
        .rev()
        .find(|(_, t)| t.starts_with(&own) || (d.step == Step::Halted && t.starts_with(restart)))
        .map(|(at, _)| *at)
}

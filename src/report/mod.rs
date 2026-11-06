//! Файл: report/mod.rs
//! Что делает: отчёты для скачивания из интерфейса. Решение 193 — «Итоги сделок» (arb_results_<дата>.xlsx) 1 в 1 как
//! отчёт 05.11.2026, на сделках из учёта ядра на момент скачивания.
//! Кто вызывает: api/report.rs.
//! От чего зависит: core (deals), report/rows.rs, report/results_xlsx.rs, report/results_sheet.rs.
//! Настройки: нет.

pub mod results_sheet;
pub mod results_xlsx;
pub mod rows;

use chrono::{DateTime, Utc};

use crate::core::deal::Deal;

/// Книга «Итоги сделок» по сделкам. Принимает: сделки, время выгрузки. Возвращает: байты .xlsx или текст ошибки.
pub fn results(deals: &[Deal], now: DateTime<Utc>) -> Result<Vec<u8>, String> {
    results_xlsx::workbook(&rows::rows(deals), now).map_err(|e| format!("отчёт xlsx: {e}"))
}

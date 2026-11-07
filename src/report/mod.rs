//! Файл: report/mod.rs
//! Что делает: отчёты для скачивания из интерфейса и Telegram. Решение 193 — «Итоги сделок» (arb_results_<дата>.xlsx)
//! 1 в 1 как отчёт 05.11.2026, на сделках из учёта ядра на момент скачивания. Решение 215 — период у «Итогов сделок»
//! (from/to по времени старта сделки; без них — за всё время, как раньше) и отчёт «Упущено» (arb_missed_<дата>.xlsx)
//! по данным экрана «Упущено из-за потолка»; файлы отчётов одной функцией на отчёт — для api/report.rs и кнопки
//! «📊 Отчёты» чата «Управление» (notify/ctl/report.rs).
//! Кто вызывает: api/report.rs, notify/ctl/report.rs.
//! От чего зависит: core (deals, uncapped_report), report/rows.rs, results_xlsx.rs, results_sheet.rs, missed_rows.rs,
//! missed_xlsx.rs, missed_total.rs.
//! Настройки: нет.

pub mod missed_rows;
pub mod missed_total;
pub mod missed_xlsx;
pub mod results_sheet;
pub mod results_xlsx;
pub mod rows;

use chrono::{DateTime, Utc};

use crate::core::Core;
use crate::core::deal::Deal;
use crate::core::uncapped_report::MissedData;

/// Тип файла .xlsx (Content-Type ответа и документа Telegram).
pub const XLSX_MIME: &str = "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet";

/// Книга «Итоги сделок» по сделкам периода [from, to) по времени старта (None — без границы). Принимает: сделки,
/// границы, время выгрузки. Возвращает: байты .xlsx или текст ошибки.
pub fn results(
    deals: &[Deal],
    from: Option<DateTime<Utc>>,
    to: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> Result<Vec<u8>, String> {
    let picked: Vec<Deal> = deals
        .iter()
        .filter(|d| in_period(d.started_at, from, to))
        .cloned()
        .collect();
    results_xlsx::workbook(&rows::rows(&picked), now).map_err(|e| format!("отчёт xlsx: {e}"))
}

/// Книга «Упущено» по данным периода. Принимает: данные (Core::uncapped_report), границы (для подписи периода),
/// время выгрузки. Возвращает: байты .xlsx или текст ошибки.
pub fn missed(
    data: &MissedData,
    from: Option<DateTime<Utc>>,
    to: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> Result<Vec<u8>, String> {
    missed_xlsx::workbook(data, &period_text(from, to), now).map_err(|e| format!("отчёт xlsx: {e}"))
}

/// Время в периоде [from, to). Ошибок нет.
pub fn in_period(t: DateTime<Utc>, from: Option<DateTime<Utc>>, to: Option<DateTime<Utc>>) -> bool {
    from.is_none_or(|f| t >= f) && to.is_none_or(|x| t < x)
}

/// Подпись периода: «за всё время», «с … UTC», «с … по … UTC». Ошибок нет.
pub fn period_text(from: Option<DateTime<Utc>>, to: Option<DateTime<Utc>>) -> String {
    let f = |t: DateTime<Utc>| t.format("%Y-%m-%d %H:%M").to_string();
    match (from, to) {
        (None, None) => "за всё время".into(),
        (Some(a), None) => format!("с {} UTC по момент выгрузки", f(a)),
        (None, Some(b)) => format!("до {} UTC", f(b)),
        (Some(a), Some(b)) => format!("с {} по {} UTC", f(a), f(b)),
    }
}

/// Имя файла отчёта: <префикс>_<ГГГГ-ММ-ДД>.xlsx (arb_results, arb_missed). Ошибок нет.
pub fn file_name(prefix: &str, now: DateTime<Utc>) -> String {
    format!("{prefix}_{}.xlsx", now.format("%Y-%m-%d"))
}

/// Файл «Итоги сделок» за период — сделки учёта ядра на момент вызова, книга в spawn_blocking.
/// Принимает: ядро, границы, время выгрузки. Возвращает: байты .xlsx. Ошибки: текст (книга не собралась).
pub async fn results_file(
    core: &Core,
    from: Option<DateTime<Utc>>,
    to: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> Result<Vec<u8>, String> {
    let deals = core.deals();
    tokio::task::spawn_blocking(move || results(&deals, from, to, now))
        .await
        .map_err(|e| format!("отчёт: задача сборки прервана: {e}"))?
}

/// Файл «Упущено» за период — данные экрана «Упущено из-за потолка», книга в spawn_blocking.
/// Принимает: ядро, границы, время выгрузки. Возвращает: байты .xlsx. Ошибки: текст (база, книга).
pub async fn missed_file(
    core: &Core,
    from: Option<DateTime<Utc>>,
    to: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> Result<Vec<u8>, String> {
    let data = core
        .uncapped_report(from, to)
        .await
        .map_err(|e| format!("отчёт «Упущено»: записи не прочитаны: {e}"))?;
    tokio::task::spawn_blocking(move || missed(&data, from, to, now))
        .await
        .map_err(|e| format!("отчёт: задача сборки прервана: {e}"))?
}

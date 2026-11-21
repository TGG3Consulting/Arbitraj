//! Файл: api/report.rs
//! Что делает: отчёты Excel для скачивания.
//! - Решение 193 — GET /api/report/results.xlsx[?from=&to=]: «Итоги сделок» (arb_results_<ГГГГ-ММ-ДД>.xlsx) на сделках
//!   учёта ядра на момент запроса. Решение 215: from/to (RFC 3339) — окно [from, to); решение 257 — сделки по времени
//!   конца, итог дня как у бота, слепок балансов (та же функция, что у Telegram: report::results_file); без них — за всё
//!   время (кнопка экрана «Сделки», ui/src/pages/Deals.svelte, их не шлёт).
//! - Решение 215 — GET /api/report/missed.xlsx[?from=&to=]: «Упущено» (arb_missed_<ГГГГ-ММ-ДД>.xlsx) по данным экрана
//!   «Упущено из-за потолка» за тот же период, что выбран на экране (ui/src/pages/Uncapped.svelte); без параметров —
//!   за всё время.
//!
//! Кто вызывает: api/server.rs (маршруты).
//! От чего зависит: report/mod.rs (results_file, missed_file, file_name, XLSX_MIME), core.
//! Настройки: нет.

use axum::body::Body;
use axum::extract::{Query, State};
use axum::http::header;
use axum::response::Response;
use chrono::{DateTime, Utc};
use serde::Deserialize;

use super::state::{ApiError, AppState};
use crate::report;

/// Период отчёта: RFC 3339 с поясом; пусто или нет — без границы.
#[derive(Deserialize, Default)]
pub struct PeriodQ {
    from: Option<String>,
    to: Option<String>,
}

/// Время запроса (RFC 3339) → UTC. Пусто — без границы. Ошибки: 400 — не время.
fn time(v: Option<&str>) -> Result<Option<DateTime<Utc>>, ApiError> {
    match v.map(str::trim) {
        None | Some("") => Ok(None),
        Some(s) => DateTime::parse_from_rfc3339(s)
            .map(|t| Some(t.with_timezone(&Utc)))
            .map_err(|e| ApiError::bad(format!("время «{s}»: {e}"))),
    }
}

/// Ответ-файл .xlsx. Принимает: байты, имя файла. Ошибки: 500 — ответ не собрался.
fn file(bytes: Vec<u8>, name: String) -> Result<Response, ApiError> {
    Response::builder()
        .header(header::CONTENT_TYPE, report::XLSX_MIME)
        .header(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{name}\""),
        )
        .body(Body::from(bytes))
        .map_err(ApiError::internal)
}

/// «Итоги сделок». Ошибки: 400 — неверное время; 500 — книга не собралась.
pub async fn results(
    State(st): State<AppState>,
    Query(q): Query<PeriodQ>,
) -> Result<Response, ApiError> {
    let now = Utc::now();
    let (from, to) = (time(q.from.as_deref())?, time(q.to.as_deref())?);
    let bytes = report::results_file(&st.core, from, to, now)
        .await
        .map_err(ApiError::internal)?;
    file(bytes, report::file_name("arb_results", now))
}

/// «Упущено». Ошибки: 400 — неверное время; 500 — база или книга.
pub async fn missed(
    State(st): State<AppState>,
    Query(q): Query<PeriodQ>,
) -> Result<Response, ApiError> {
    let now = Utc::now();
    let (from, to) = (time(q.from.as_deref())?, time(q.to.as_deref())?);
    let bytes = report::missed_file(&st.core, from, to, now)
        .await
        .map_err(ApiError::internal)?;
    file(bytes, report::file_name("arb_missed", now))
}

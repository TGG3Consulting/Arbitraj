//! Файл: api/report.rs
//! Что делает: решение 193 — GET /api/report/results.xlsx: отчёт «Итоги сделок» (arb_results_<ГГГГ-ММ-ДД>.xlsx) 1 в 1
//! как отчёт 05.11.2026, на сделках учёта ядра на момент запроса. Кнопка — экран «Сделки» (ui/src/pages/Deals.svelte).
//! Кто вызывает: api/server.rs (маршрут).
//! От чего зависит: core (deals), report/mod.rs.
//! Настройки: нет.

use axum::body::Body;
use axum::extract::State;
use axum::http::header;
use axum::response::Response;
use chrono::Utc;

use super::state::{ApiError, AppState};

/// Файл отчёта. Ошибки: 500 — книга не собралась.
pub async fn results(State(st): State<AppState>) -> Result<Response, ApiError> {
    let now = Utc::now();
    let deals = st.core.deals();
    let bytes = tokio::task::spawn_blocking(move || crate::report::results(&deals, now))
        .await
        .map_err(ApiError::internal)?
        .map_err(ApiError::internal)?;
    Response::builder()
        .header(
            header::CONTENT_TYPE,
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        )
        .header(
            header::CONTENT_DISPOSITION,
            format!(
                "attachment; filename=\"arb_results_{}.xlsx\"",
                now.format("%Y-%m-%d")
            ),
        )
        .body(Body::from(bytes))
        .map_err(ApiError::internal)
}

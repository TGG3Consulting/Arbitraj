//! Файл: api/journal.rs
//! Что делает: REST журнала — чтение записей, новые первыми, с листанием.
//! Кто вызывает: интерфейс (экран «Журнал»).
//! От чего зависит: api/state.rs, storage/journal.rs.
//! Настройки: нет. Размер страницы задаёт интерфейс параметром limit (обязателен).
//!
//! GET /api/journal?limit=N[&before=ID]

use axum::Json;
use axum::extract::{Query, State};
use serde::Deserialize;

use crate::storage::journal::Entry;

use super::state::{ApiError, AppState};

/// Параметры чтения.
#[derive(Deserialize)]
pub struct JournalQuery {
    limit: u32,
    before: Option<i64>,
}

/// Записи журнала. Ошибки: 400 — не указан limit; 500 — база.
pub async fn list(
    State(st): State<AppState>,
    Query(q): Query<JournalQuery>,
) -> Result<Json<Vec<Entry>>, ApiError> {
    st.journal
        .list(q.limit, q.before)
        .await
        .map(Json)
        .map_err(ApiError::internal)
}

//! Файл: api/rules.rs
//! Что делает: REST правил шлюза — список, создание, изменение (выключение = enabled:false).
//! Каждое изменение пишется в журнал.
//! Кто вызывает: интерфейс (экран «Правила»).
//! От чего зависит: api/state.rs, storage/rules.rs, gateway/rules.rs, storage/journal.
//! Настройки: нет (правила — сами по себе данные из интерфейса).
//!
//! GET /api/rules        — все правила
//! POST /api/rules       — создать (id игнорируется)
//! PUT /api/rules/{id}   — заменить правило

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde_json::json;

use crate::gateway::rules::Rule;
use crate::storage::journal::Level;
use crate::storage::rules as store;

use super::state::{ApiError, AppState};

/// Все правила. Ошибки: 500 — база.
pub async fn list(State(st): State<AppState>) -> Result<Json<Vec<Rule>>, ApiError> {
    store::list(&st.db)
        .await
        .map(Json)
        .map_err(ApiError::internal)
}

/// Создаёт правило. Возвращает: правило с id. Ошибки: 500 — база.
pub async fn create(
    State(st): State<AppState>,
    Json(rule): Json<Rule>,
) -> Result<Json<Rule>, ApiError> {
    let saved = store::create(&st.db, rule)
        .await
        .map_err(ApiError::internal)?;
    let _ = st
        .journal
        .write(
            Level::Info,
            "rules.created",
            None,
            format!("Создано правило №{}", saved.id),
            Some(json!(saved)),
        )
        .await;
    Ok(Json(saved))
}

/// Заменяет правило. Ошибки: 404 — нет такого правила; 500 — база.
pub async fn update(
    State(st): State<AppState>,
    Path(id): Path<i64>,
    Json(mut rule): Json<Rule>,
) -> Result<Json<Rule>, ApiError> {
    rule.id = id;
    let found = store::update(&st.db, rule.clone())
        .await
        .map_err(ApiError::internal)?;
    if !found {
        return Err(ApiError(
            StatusCode::NOT_FOUND,
            format!("Правила №{id} нет"),
        ));
    }
    let _ = st
        .journal
        .write(
            Level::Info,
            "rules.updated",
            None,
            format!("Изменено правило №{id}"),
            Some(json!(rule)),
        )
        .await;
    Ok(Json(rule))
}

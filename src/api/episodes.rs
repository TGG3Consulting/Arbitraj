//! Файл: api/episodes.rs
//! Что делает: экран «Эпизоды» (решение 74) — список эпизодов арбитражной ситуации, записи одного эпизода и скачивание
//! его файла zstd.
//! Кто вызывает: интерфейс (ui/src/pages/Episodes.svelte).
//! От чего зависит: api/state.rs, core (Core::episodes, episode, episode_path — core/episode_view.rs).
//! Настройки: нет (episode.* — экран «Настройки»). Сколько строк и записей — параметр limit (обязателен).
//!
//! GET /api/episodes?limit=N           — {episodes: [EpisodeInfo], off: причина | null, used_bytes}
//! GET /api/episodes/{id}?limit=N      — {info, records: [запись JSON], truncated}; 404 — нет эпизода
//! GET /api/episodes/{id}/file         — файл <id>.jsonl.zst (application/zstd, attachment)

use axum::Json;
use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{StatusCode, header};
use axum::response::Response;
use serde::Deserialize;
use serde_json::Value;

use super::state::{ApiError, AppState};

/// Сколько строк или записей отдать.
#[derive(Deserialize)]
pub struct Limit {
    limit: u32,
}

/// Список эпизодов. Ошибки: 400 — нет limit; 500 — база.
pub async fn list(
    State(st): State<AppState>,
    Query(q): Query<Limit>,
) -> Result<Json<Value>, ApiError> {
    st.core
        .episodes(q.limit)
        .await
        .map(Json)
        .map_err(ApiError::internal)
}

/// Записи эпизода. Ошибки: 404 — нет эпизода; 500 — база или файл.
pub async fn get(
    State(st): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<Limit>,
) -> Result<Json<Value>, ApiError> {
    match st.core.episode(&id, q.limit as usize).await {
        Ok(Some(v)) => Ok(Json(v)),
        Ok(None) => Err(ApiError(StatusCode::NOT_FOUND, format!("эпизода {id} нет"))),
        Err(e) => Err(ApiError::internal(e)),
    }
}

/// Файл эпизода как есть (сжатый). Ошибки: 400 — неверный id; 404 — файла нет (удалён по сроку или месту).
pub async fn file(
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let path = st
        .core
        .episode_path(&id)
        .ok_or_else(|| ApiError::bad(format!("неверный id эпизода «{id}»")))?;
    let bytes = tokio::fs::read(&path)
        .await
        .map_err(|e| ApiError(StatusCode::NOT_FOUND, format!("файл эпизода {id}: {e}")))?;
    Response::builder()
        .header(header::CONTENT_TYPE, "application/zstd")
        .header(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{id}.jsonl.zst\""),
        )
        .body(Body::from(bytes))
        .map_err(ApiError::internal)
}

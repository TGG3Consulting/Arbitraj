//! Файл: api/windows.rs
//! Что делает: экран «Окна» (решение 125) — страница списка окон с фильтрами, кадры одного окна (стакан по времени и
//! лента сделок, разобранные адаптером площадки) и скачивание файла окна zstd как есть.
//! Кто вызывает: интерфейс (ui/src/pages/Windows.svelte, ui/src/components/WindowView.svelte).
//! От чего зависит: api/state.rs, core (Core::windows, window, window_path — core/window_view.rs),
//! storage/windows_db.rs (Filter).
//! Настройки: нет (window.* — экран «Настройки»). Сколько строк и кадров — параметр limit (обязателен).
//!
//! GET /api/windows?limit=N&offset=M&coin=T&deal=yes|no&outcome=plus|minus|zero (coin — тикер или адрес 0x…, решение 211)
//!                                 — {windows: [WindowInfo], total, off: причина | null, used_bytes}
//! GET /api/windows/{id}?limit=N&offset=M&levels=K — {info, lines, frames (у изменений стакана — book: стакан после кадра,
//!                                 K верхних уровней), frames_read, truncated, deals}; 404 — нет окна
//! GET /api/windows/{id}/file      — файл <id>.jsonl.zst (application/zstd, attachment)

use axum::Json;
use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{StatusCode, header};
use axum::response::Response;
use serde::Deserialize;
use serde_json::Value;

use crate::storage::windows_db::Filter;

use super::state::{ApiError, AppState};

/// Параметры списка.
#[derive(Deserialize)]
pub struct ListQ {
    limit: u32,
    #[serde(default)]
    offset: u32,
    coin: Option<String>,
    deal: Option<String>,
    outcome: Option<String>,
}

/// Параметры кадров окна.
#[derive(Deserialize)]
pub struct GetQ {
    limit: usize,
    /// Сколько верхних уровней стакана всей глубины показать на кадр изменений.
    levels: usize,
    #[serde(default)]
    offset: usize,
}

/// Список окон. Ошибки: 400 — нет limit или неверный фильтр; 500 — база.
pub async fn list(
    State(st): State<AppState>,
    Query(q): Query<ListQ>,
) -> Result<Json<Value>, ApiError> {
    let deal = match q.deal.as_deref() {
        None | Some("") => None,
        Some("yes") => Some(true),
        Some("no") => Some(false),
        Some(o) => return Err(ApiError::bad(format!("deal = «{o}»: только yes или no"))),
    };
    let outcome = match q.outcome.as_deref() {
        None | Some("") => None,
        Some(o @ ("plus" | "minus" | "zero")) => Some(o.to_string()),
        Some(o) => {
            return Err(ApiError::bad(format!(
                "outcome = «{o}»: только plus, minus или zero"
            )));
        }
    };
    // Решение 211: монета — адрес контракта (0x…, без учёта регистра) или тикер (как раньше).
    let coin = q
        .coin
        .map(|c| c.trim().to_string())
        .filter(|c| !c.is_empty());
    let (ticker, address) = match coin {
        Some(c) if crate::settings::scope::is_address_id(&c) => (None, Some(c.to_lowercase())),
        Some(c) => (Some(c.to_uppercase()), None),
        None => (None, None),
    };
    let f = Filter {
        ticker,
        address,
        deal,
        outcome,
    };
    st.core
        .windows(f, q.limit, q.offset)
        .await
        .map(Json)
        .map_err(ApiError::internal)
}

/// Окно и его кадры. Ошибки: 404 — нет окна; 500 — база или файл.
pub async fn get(
    State(st): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<GetQ>,
) -> Result<Json<Value>, ApiError> {
    match st.core.window(&id, q.offset, q.limit, q.levels).await {
        Ok(Some(v)) => Ok(Json(v)),
        Ok(None) => Err(ApiError(StatusCode::NOT_FOUND, format!("окна {id} нет"))),
        Err(e) => Err(ApiError::internal(e)),
    }
}

/// Файл окна как есть (сжатый). Ошибки: 400 — неверный id; 404 — файла нет (удалён по сроку или месту).
pub async fn file(
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let path = st
        .core
        .window_path(&id)
        .ok_or_else(|| ApiError::bad(format!("неверный id окна «{id}»")))?;
    let bytes = tokio::fs::read(&path)
        .await
        .map_err(|e| ApiError(StatusCode::NOT_FOUND, format!("файл окна {id}: {e}")))?;
    Response::builder()
        .header(header::CONTENT_TYPE, "application/zstd")
        .header(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{id}.jsonl.zst\""),
        )
        .body(Body::from(bytes))
        .map_err(ApiError::internal)
}

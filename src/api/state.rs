//! Файл: api/state.rs
//! Что делает: общее состояние веб-сервера и единый формат ошибки ответа.
//! Кто вызывает: все обработчики api/*.
//! От чего зависит: settings, storage, gateway, notify, secrets.
//! Настройки: нет.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;
use tokio::sync::broadcast;

use crate::gateway::registry::Registry;
use crate::notify::dispatch::Notifier;
use crate::secrets::Vault;
use crate::settings::live::Change;
use crate::settings::store::Store;
use crate::storage::db::Db;
use crate::storage::journal::{Entry, Journal};

/// Состояние, общее для всех обработчиков.
#[derive(Clone)]
pub struct AppState {
    pub db: Db,
    pub settings: Store,
    pub journal: Journal,
    pub registry: Registry,
    pub vault: Vault,
    pub notifier: Notifier,
    pub http: reqwest::Client,
    /// Поток изменений настроек — для WebSocket интерфейса.
    pub changes: broadcast::Sender<Change>,
    /// Поток записей журнала — для WebSocket интерфейса.
    pub journal_live: broadcast::Sender<Entry>,
}

/// Ошибка ответа: HTTP-код и понятный текст по-русски.
pub struct ApiError(pub StatusCode, pub String);

impl IntoResponse for ApiError {
    /// Превращает ошибку в JSON {"error": "..."} с нужным кодом.
    fn into_response(self) -> Response {
        (self.0, Json(json!({ "error": self.1 }))).into_response()
    }
}

impl ApiError {
    /// 400 — неверный запрос.
    pub fn bad(msg: impl Into<String>) -> Self {
        Self(StatusCode::BAD_REQUEST, msg.into())
    }

    /// 500 — внутренняя ошибка (подробности — в лог процесса).
    pub fn internal(err: impl std::fmt::Display) -> Self {
        tracing::error!(%err, "ошибка обработчика API");
        Self(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Внутренняя ошибка: {err}"),
        )
    }
}

/// Кто меняет настройки через интерфейс. Авторизация веб-сервера — открытый вопрос (ARCHITECTURE.md).
pub const CHANGED_BY_UI: &str = "интерфейс";

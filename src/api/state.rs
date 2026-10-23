//! Файл: api/state.rs
//! Что делает: общее состояние веб-сервера и единый формат ошибки ответа.
//! Кто вызывает: все обработчики api/*.
//! От чего зависит: settings, storage, gateway, notify, secrets, core (Core).
//! Настройки: нет.

use std::sync::Arc;

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;
use tokio::sync::{Notify, broadcast};

use crate::core::Core;
use crate::gateway::errors::{ErrorCode, GatewayError};
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
    /// Ядро: состояние экранов, живой поток (Core::subscribe), команды человека и управление.
    pub core: Core,
    /// Сигнал «секреты изменились» (создание, разблокировка, смена, удаление, блокировка) для приёма
    /// кнопок Telegram (notify/updates.rs, Updates::wake).
    pub secrets_wake: Arc<Notify>,
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

    /// 503 — нет данных: источника ещё нет (например ядро не подключено к веб-серверу).
    /// Интерфейс показывает «нет данных» с этой причиной; выдуманных чисел нет (CLAUDE.md).
    pub fn no_data(msg: impl Into<String>) -> Self {
        Self(StatusCode::SERVICE_UNAVAILABLE, msg.into())
    }

    /// Ошибка шлюза → ответ интерфейсу (src/api/README.md):
    /// NotApplicable — 400 «не применимо» (функция не для этой площадки, например список DEX у биржи);
    /// VenueNotConnected — 404 (площадки нет в реестре шлюза); остальное — 502 (сбой площадки).
    /// Текст — сообщение шлюза как есть, с площадкой.
    pub fn gateway(e: GatewayError) -> Self {
        let status = match e.code {
            ErrorCode::NotApplicable => StatusCode::BAD_REQUEST,
            ErrorCode::VenueNotConnected => StatusCode::NOT_FOUND,
            _ => StatusCode::BAD_GATEWAY,
        };
        let text = match &e.venue {
            Some(v) => format!("{} ({}): {}", v.0, code_name(e.code), e.message),
            None => format!("{}: {}", code_name(e.code), e.message),
        };
        Self(status, text)
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

/// Имя единого кода ошибки, как его сериализует serde (например «NotApplicable»).
/// Принимает: код. Возвращает: строку. Ошибок нет.
fn code_name(code: ErrorCode) -> String {
    serde_json::to_value(code)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_else(|| format!("{code:?}"))
}

/// Кто меняет настройки через интерфейс. Авторизация веб-сервера — открытый вопрос (ARCHITECTURE.md).
pub const CHANGED_BY_UI: &str = "интерфейс";

#[cfg(test)]
mod tests {
    use super::*;

    /// Ошибка шлюза получает HTTP-код по контракту src/api/README.md («не применимо» — 400).
    #[test]
    fn gateway_error_status() {
        let v = Some(crate::core::types::VenueId("bingx".into()));
        let na = ApiError::gateway(GatewayError::new(ErrorCode::NotApplicable, v.clone(), "x"));
        assert_eq!(na.0, StatusCode::BAD_REQUEST);
        assert_eq!(na.1, "bingx (NotApplicable): x");
        let nc = ApiError::gateway(GatewayError::new(ErrorCode::VenueNotConnected, v, "y"));
        assert_eq!(nc.0, StatusCode::NOT_FOUND);
        let other = ApiError::gateway(GatewayError::new(ErrorCode::Timeout, None, "z"));
        assert_eq!(other.0, StatusCode::BAD_GATEWAY);
        assert_eq!(other.1, "Timeout: z");
    }
}

//! Файл: api/server.rs
//! Что делает: маршруты веб-сервера и его запуск.
//! Кто вызывает: main.rs.
//! От чего зависит: axum, api/*.
//! Настройки: адрес прослушивания — аргумент запуска --listen (нужен до загрузки настроек).
//!
//! Авторизация доступа к веб-серверу — открытый вопрос (ARCHITECTURE.md, «Открытые вопросы»).
//! До решения запускать только на локальном адресе ПК (например 127.0.0.1).

use axum::Router;
use axum::routing::{get, post, put};
use tokio::net::TcpListener;

use super::state::AppState;
use super::{check, journal, rules, settings, ws};

/// Собирает все маршруты API.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/settings/schema", get(settings::schema))
        .route("/api/settings", get(settings::values).put(settings::set))
        .route("/api/settings/history", get(settings::history))
        .route("/api/rules", get(rules::list).post(rules::create))
        .route("/api/rules/{id}", put(rules::update))
        .route("/api/journal", get(journal::list))
        .route("/api/check/telegram", post(check::telegram))
        .route("/api/check/venue/{id}", post(check::venue))
        .route("/api/ws", get(ws::upgrade))
        .with_state(state)
}

/// Запускает сервер и работает до остановки процесса.
/// Принимает: открытый слушатель и состояние. Ошибки: сбой сервера.
pub async fn serve(listener: TcpListener, state: AppState) -> std::io::Result<()> {
    axum::serve(listener, router(state)).await
}

//! Файл: api/server.rs
//! Что делает: маршруты веб-сервера и его запуск; если задан --ui-dir — ещё и раздача
//! собранного интерфейса (api/static_ui.rs).
//! Кто вызывает: main.rs.
//! От чего зависит: axum, api/*.
//! Настройки: адрес прослушивания и папка интерфейса — аргументы запуска --listen и --ui-dir
//! (нужны до загрузки настроек).
//!
//! Авторизация доступа к веб-серверу — открытый вопрос (ARCHITECTURE.md, «Открытые вопросы»).
//! До решения запускать только на локальном адресе ПК (например 127.0.0.1).

use std::path::PathBuf;

use axum::Router;
use axum::routing::{any, get, post, put};
use tokio::net::TcpListener;

use super::guard::{self, Guard};
use super::state::AppState;
use super::{
    check, coins, control, core_api, dex, episodes, journal, rules, rules_meta, secrets, settings,
    static_ui, venues, wallet_queue, ws,
};

/// Собирает все маршруты API и, если задана папка, раздачу интерфейса — всё под защитой
/// Host/Origin (api/guard.rs).
/// Принимает: состояние, защиту, папку собранного интерфейса (None — только API).
pub fn router(state: AppState, guard: Guard, ui_dir: Option<PathBuf>) -> Router {
    let api = Router::new()
        .route("/api/settings/schema", get(settings::schema))
        .route("/api/settings", get(settings::values).put(settings::set))
        .route("/api/settings/history", get(settings::history))
        .route("/api/rules", get(rules::list).post(rules::create))
        .route("/api/rules/meta", get(rules_meta::meta))
        .route("/api/rules/{id}", put(rules::update))
        .route("/api/venues", get(venues::list))
        .route("/api/venues/kinds", get(venues::kinds))
        .route("/api/venues/{id}/dex-families", get(dex::families))
        .route("/api/venues/{id}/pool-directory", get(dex::directory))
        .route("/api/channels/{venue}", get(dex::channels))
        .route("/api/wallet-queue/{venue}", get(wallet_queue::get))
        .route(
            "/api/wallet-queue/{venue}/release",
            post(wallet_queue::release),
        )
        .route("/api/coins", get(coins::list))
        .route("/api/coins/recheck", post(coins::recheck))
        .route("/api/manual", get(control::manual))
        .route("/api/manual/command", post(control::command))
        .route("/api/gas-stats", get(control::gas_stats))
        .route("/api/spreads", get(core_api::spreads))
        .route("/api/candidates", get(core_api::candidates))
        .route("/api/deals", get(core_api::deals))
        .route("/api/dry", get(core_api::dry))
        .route("/api/transits", get(core_api::transits))
        .route(
            "/api/control",
            get(core_api::control).post(core_api::set_mode),
        )
        .route(
            "/api/control/venue/{id}/resume",
            post(core_api::resume_venue),
        )
        .route(
            "/api/control/quarantine/{ticker}/release",
            post(core_api::release),
        )
        .route("/api/episodes", get(episodes::list))
        .route("/api/episodes/{id}", get(episodes::get))
        .route("/api/episodes/{id}/file", get(episodes::file))
        .route("/api/journal", get(journal::list))
        .route("/api/check/telegram", post(check::telegram))
        .route("/api/check/venue/{id}", post(check::venue))
        .route("/api/check/channel/{venue}/{id}", post(check::channel))
        .route("/api/secrets/known", get(secrets::known))
        .route("/api/secrets/status", get(secrets::status))
        .route("/api/secrets/create", post(secrets::create))
        .route("/api/secrets/unlock", post(secrets::unlock))
        .route("/api/secrets/lock", post(secrets::lock))
        .route("/api/secrets/password", post(secrets::password))
        .route(
            "/api/secrets/{name}",
            put(secrets::put).delete(secrets::delete),
        )
        .route("/api/ws", get(ws::upgrade))
        // Неизвестный адрес API — 404 с причиной, а не страница интерфейса.
        .route("/api/{*rest}", any(static_ui::api_not_found))
        .with_state(state);
    let app = match ui_dir {
        Some(dir) => api.fallback_service(static_ui::service(&dir)),
        None => api,
    };
    app.layer(axum::middleware::from_fn_with_state(guard, guard::check))
}

/// Запускает сервер и работает до остановки процесса.
/// Принимает: открытый слушатель, состояние, защиту, папку интерфейса (None — только API).
/// Ошибки: сбой сервера.
pub async fn serve(
    listener: TcpListener,
    state: AppState,
    guard: Guard,
    ui_dir: Option<PathBuf>,
) -> std::io::Result<()> {
    axum::serve(listener, router(state, guard, ui_dir)).await
}

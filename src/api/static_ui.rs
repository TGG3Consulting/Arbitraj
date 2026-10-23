//! Файл: api/static_ui.rs
//! Что делает: раздача собранного интерфейса (ui/dist) на «/». Любой адрес, которого нет
//! среди файлов, получает index.html — экраны одностраничного приложения открываются по прямой ссылке.
//! Неизвестный адрес под /api/ — 404 с причиной (а не страница интерфейса).
//! Кто вызывает: api/server.rs (только если задан аргумент --ui-dir).
//! От чего зависит: tower-http (ServeDir, ServeFile), api/state.rs.
//! Настройки: нет; папка — аргумент запуска --ui-dir (cli.rs).

use std::path::Path;

use axum::http::{StatusCode, Uri};
use tower_http::services::{ServeDir, ServeFile};

use super::state::ApiError;

/// Имя главной страницы собранного интерфейса (так её называет сборщик Vite).
const INDEX: &str = "index.html";

/// Сервис раздачи файлов интерфейса.
/// Принимает: папку собранного интерфейса. Возвращает: сервис для fallback маршрутизатора.
/// Ошибки: нет при создании; нет файла — отдаётся index.html, нет и его — 404.
pub fn service(dir: &Path) -> ServeDir<ServeFile> {
    ServeDir::new(dir).fallback(ServeFile::new(dir.join(INDEX)))
}

/// Ответ на неизвестный адрес API.
/// Принимает: адрес запроса. Возвращает: всегда ошибку 404 {"error": "..."}.
pub async fn api_not_found(uri: Uri) -> ApiError {
    ApiError(
        StatusCode::NOT_FOUND,
        format!("Нет такого адреса API: {uri}"),
    )
}

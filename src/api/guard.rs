//! Файл: api/guard.rs
//! Что делает: защита веб-сервера от чужих страниц в браузере на том же ПК.
//! Запрос принимается, только если заголовок Host — это адрес --listen, а заголовок Origin
//! (если браузер его прислал) — это сам интерфейс или адрес из --allow-origin.
//! Защищает от DNS-rebinding и от чужих сайтов, открывающих ws://127.0.0.1.
//! Кто вызывает: api/server.rs (слой для всех маршрутов).
//! От чего зависит: axum.
//! Настройки: нет; адреса — из аргументов запуска.
//!
//! Это не авторизация: вход по паролю — открытый вопрос (ARCHITECTURE.md).

use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::{StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

/// Разрешённые Host и Origin.
#[derive(Clone)]
pub struct Guard {
    host: Arc<String>,
    origins: Arc<Vec<String>>,
}

impl Guard {
    /// Создаёт защиту.
    /// Принимает: адрес --listen («127.0.0.1:8080») и дополнительные Origin из --allow-origin.
    pub fn new(listen: &str, extra_origins: &[String]) -> Self {
        let mut origins = vec![format!("http://{listen}")];
        origins.extend(extra_origins.iter().cloned());
        Self {
            host: Arc::new(listen.to_string()),
            origins: Arc::new(origins),
        }
    }
}

/// Проверяет запрос. Чужой Host или Origin → 403 с причиной; иначе запрос идёт дальше.
pub async fn check(State(g): State<Guard>, req: Request, next: Next) -> Response {
    if let Err(reason) = allowed(&g, &req) {
        return (StatusCode::FORBIDDEN, reason).into_response();
    }
    next.run(req).await
}

/// Сверяет Host и Origin с разрешёнными. Возвращает Err с причиной отказа.
fn allowed(g: &Guard, req: &Request) -> Result<(), &'static str> {
    let get = |name| req.headers().get(name).and_then(|v| v.to_str().ok());
    if get(header::HOST) != Some(g.host.as_str()) {
        return Err("Запрос не к адресу интерфейса (Host)");
    }
    match get(header::ORIGIN) {
        Some(origin) if !g.origins.iter().any(|o| o == origin) => {
            Err("Страница не из разрешённых (Origin)")
        }
        _ => Ok(()),
    }
}

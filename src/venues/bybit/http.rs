//! Файл: venues/bybit/http.rs
//! Что делает: один HTTP-обмен с Bybit с теми же сроками и пределами, что gateway/http.rs (http.timeout_ms до
//! заголовков, http.body_idle_ms на порцию тела, http.total_ms на весь обмен, http.max_body_bytes), и с заголовками
//! лимита Bybit X-Bapi-Limit, X-Bapi-Limit-Status, X-Bapi-Limit-Reset-Timestamp — их общий обмен шлюза не отдаёт, а
//! без них нельзя остановить запросы до сброса лимита (урок 15). Ошибки — те же Fail шлюза.
//! Кто вызывает: rest.rs.
//! От чего зависит: gateway/http.rs (Fail, Limits), reqwest, tokio.
//! Настройки: нет своих (сроки передаёт rest.rs из http.*).
//!
//! Официальная документация «Rate Limit Rules» https://bybit-exchange.github.io/docs/v5/rate-limit: «Every request to
//! the API returns response header…: X-Bapi-Limit-Status — your remaining requests for current endpoint; X-Bapi-Limit —
//! your current limit for current endpoint; X-Bapi-Limit-Reset-Timestamp — the timestamp indicating when your request
//! limit resets if you have exceeded your rate_limit».

use std::sync::atomic::{AtomicUsize, Ordering};

use reqwest::{RequestBuilder, StatusCode};

use crate::gateway::http::{Fail, Limits};

/// Ответ Bybit: код, тело и заголовки лимита.
#[derive(Debug, Clone)]
pub struct Reply {
    pub status: StatusCode,
    pub text: String,
    /// Предел запросов эндпоинта (X-Bapi-Limit).
    pub limit: Option<u64>,
    /// Сколько запросов осталось (X-Bapi-Limit-Status).
    pub left: Option<u64>,
    /// Когда лимит сбросится, Unix мс (X-Bapi-Limit-Reset-Timestamp).
    pub reset_ms: Option<i64>,
}

/// Отправляет запрос и дочитывает ответ. Принимает: запрос, сроки. Возвращает: Reply. Ошибки: Fail.
pub async fn exchange(req: RequestBuilder, lim: Limits) -> Result<Reply, Fail> {
    let got = AtomicUsize::new(0);
    match tokio::time::timeout(lim.total, read(req, lim, &got)).await {
        Ok(r) => r,
        Err(_) => Err(Fail::Total {
            got: got.load(Ordering::Relaxed),
        }),
    }
}

/// Сам обмен без общего предела (как gateway/http.rs, плюс заголовки лимита).
async fn read(req: RequestBuilder, lim: Limits, got: &AtomicUsize) -> Result<Reply, Fail> {
    let mut resp = match tokio::time::timeout(lim.head, req.send()).await {
        Err(_) => return Err(Fail::NoHead),
        Ok(Err(e)) if e.is_connect() => return Err(Fail::Connect(e.without_url().to_string())),
        Ok(Err(e)) if e.is_timeout() => return Err(Fail::NoHead),
        Ok(Err(e)) => return Err(Fail::Send(e.without_url().to_string())),
        Ok(Ok(r)) => r,
    };
    let status = resp.status();
    let head = |name: &str| {
        resp.headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(|v| v.trim().to_string())
    };
    let limit = head("X-Bapi-Limit").and_then(|v| v.parse().ok());
    let left = head("X-Bapi-Limit-Status").and_then(|v| v.parse().ok());
    let reset_ms = head("X-Bapi-Limit-Reset-Timestamp").and_then(|v| v.parse().ok());
    let mut body = Vec::new();
    loop {
        match tokio::time::timeout(lim.idle, resp.chunk()).await {
            Err(_) => return Err(Fail::Stalled { got: body.len() }),
            Ok(Err(e)) => {
                let why = e.without_url().to_string();
                return Err(Fail::Broken {
                    got: body.len(),
                    why,
                });
            }
            Ok(Ok(Some(part))) => {
                body.extend_from_slice(&part);
                got.store(body.len(), Ordering::Relaxed);
                if body.len() > lim.max_bytes {
                    return Err(Fail::TooBig { got: body.len() });
                }
            }
            Ok(Ok(None)) => break,
        }
    }
    String::from_utf8(body)
        .map(|text| Reply {
            status,
            text,
            limit,
            left,
            reset_ms,
        })
        .map_err(|_| Fail::NotText)
}

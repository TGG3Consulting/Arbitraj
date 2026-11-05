//! Файл: venues/coinex/rest.rs
//! Что делает: запросы REST к CoinEx API v2 — публичные и подписанные (sign.rs), разбор ответа (reply.rs), пауза после
//! лимита, переход на резервный адрес только при сбое сети. Большой ответ (справочники монет) разбирается вне рабочих
//! потоков tokio (решение 131). Сжатие gzip — у общего HTTP-клиента (решение 142, reqwest с gzip).
//! Кто вызывает: файлы адаптера CoinEx.
//! От чего зависит: ctx.rs, sign.rs, reply.rs, errors.rs, gateway/http.rs, reqwest.
//! Настройки: venue.rest_url, venue.rest_fallback_url, venue.recv_window_ms (необязательна) [coinex], http.*;
//! секреты coinex.*.
//!
//! Официально:
//! - Integration Guide (https://docs.coinex.com/api/v2/guide): REST «https://api.coinex.com/v2»; X-COINEX-TIMESTAMP
//!   обязателен; X-COINEX-WINDOWTIME — «time window for request validity», по умолчанию 5000 мс (не задан
//!   venue.recv_window_ms — заголовок не шлётся, действует окно CoinEx).
//! - Authentication: подписывается «Request path, including the query string» — в примере «/v2/spot/pending-order?…»,
//!   то есть путь С «/v2». Поэтому venue.rest_url — без «/v2» (https://api.coinex.com), пути в коде — с «/v2/…», и одна и
//!   та же строка идёт в адрес и в подпись. Порядок и кодирование параметров не описаны: параметры уходят в порядке
//!   кода, значения — только из безопасных знаков (буквы, цифры, «_-.,»), иначе ошибка до отправки.
//! - Content-Type тела не указан; тела POST в примерах — JSON → «application/json».

use reqwest::Method;
use serde_json::Value;

use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::Ctx;
use super::errors::{self, Op};
use super::{reply, sign};

/// Параметры строки запроса: пары ключ — значение (в этом порядке).
pub type Params = Vec<(&'static str, String)>;

/// Один запрос.
pub struct Req<'a> {
    pub method: Method,
    /// Путь с версией, без строки запроса (например /v2/spot/order).
    pub path: &'a str,
    pub query: Params,
    /// Тело JSON (только POST).
    pub body: Option<Value>,
    pub op: Op,
    /// Подписанный запрос.
    pub signed: bool,
    /// Разбор ответа вне рабочих потоков (большой ответ).
    pub heavy: bool,
}

/// Публичный GET. Принимает: путь, параметры. Возвращает: data ответа. Ошибки: связь, коды CoinEx.
pub async fn public(ctx: &Ctx, path: &str, query: Params) -> GatewayResult<Value> {
    let r = Req {
        method: Method::GET,
        path,
        query,
        body: None,
        op: Op::Other,
        signed: false,
        heavy: false,
    };
    Ok(data(send(ctx, r).await?))
}

/// Подписанный запрос. Принимает: метод, путь, параметры строки, тело, операцию.
/// Возвращает: data ответа. Ошибки: SecretsLocked/NotConfigured (ключи), коды CoinEx.
pub async fn signed(
    ctx: &Ctx,
    method: Method,
    path: &str,
    query: Params,
    body: Option<Value>,
    op: Op,
) -> GatewayResult<Value> {
    let r = Req {
        method,
        path,
        query,
        body,
        op,
        signed: true,
        heavy: false,
    };
    Ok(data(send(ctx, r).await?))
}

/// Подписанный GET страницы истории. Возвращает: (data, pagination.has_next; нет поля — false).
/// Ошибки: как signed.
pub async fn signed_page(ctx: &Ctx, path: &str, query: Params) -> GatewayResult<(Value, bool)> {
    let r = Req {
        method: Method::GET,
        path,
        query,
        body: None,
        op: Op::Other,
        signed: true,
        heavy: false,
    };
    let body = send(ctx, r).await?;
    let next = body
        .get("pagination")
        .and_then(|p| p.get("has_next"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    Ok((data(body), next))
}

/// Поле data тела (забирается без копии). Нет — Null. Ошибок нет.
pub fn data(mut body: Value) -> Value {
    body.as_object_mut()
        .and_then(|m| m.remove("data"))
        .unwrap_or(Value::Null)
}

/// Что делает: отправка запроса на адреса venue.rest_url (+ резерв только при сбое сети).
/// Принимает: окружение, запрос. Возвращает: всё тело ответа. Ошибки: как public / signed.
pub async fn send(ctx: &Ctx, r: Req<'_>) -> GatewayResult<Value> {
    let urls = ctx.rest_urls()?;
    let q = query(&r.query).map_err(|e| ctx.tag(e))?;
    let path_q = if q.is_empty() {
        r.path.to_string()
    } else {
        format!("{}?{q}", r.path)
    };
    let body = r.body.as_ref().map(Value::to_string).unwrap_or_default();
    let lim = ctx.limits()?;
    let window = ctx.int("venue.recv_window_ms").ok();
    // Пауза после лимита — запрос не отправляется вовсе.
    reply::gate().map_err(|e| ctx.tag(e))?;
    let mut last = None;
    for base in &urls {
        let mut req = ctx
            .http
            .request(r.method.clone(), format!("{base}{path_q}"));
        if r.method == Method::POST {
            req = req.header("Content-Type", "application/json");
        }
        if r.signed {
            let keys = ctx.keys()?;
            let ts = chrono::Utc::now().timestamp_millis();
            let h = sign::headers(&keys, r.method.as_str(), &path_q, &body, ts)
                .map_err(|e| ctx.tag(e))?;
            req = req
                .header("X-COINEX-KEY", h.key)
                .header("X-COINEX-SIGN", h.sign)
                .header("X-COINEX-TIMESTAMP", h.timestamp);
            if let Some(w) = window {
                req = req.header("X-COINEX-WINDOWTIME", w.to_string());
            }
        }
        if !body.is_empty() {
            req = req.body(body.clone());
        }
        match crate::gateway::http::exchange_full(req, lim).await {
            Ok(reply) => {
                let parsed = if r.heavy {
                    let (c, p, op) = (ctx.clone(), r.path.to_string(), r.op);
                    tokio::task::spawn_blocking(move || reply::parse(&c, &p, &reply, op))
                        .await
                        .map_err(|e| {
                            ctx.tag(GatewayError::new(
                                ErrorCode::Internal,
                                None,
                                format!("разбор ответа {} прерван: {e}", r.path),
                            ))
                        })?
                } else {
                    reply::parse(ctx, r.path, &reply, r.op)
                };
                return parsed.map_err(|e| errors::unsure(e, r.op));
            }
            // Резерв — только если запрос точно не дошёл; иначе ордер или вывод мог исполниться — не повторяем.
            Err(f) if f.not_sent() || (r.op == Op::Other && f.is_timeout()) => {
                last = Some(errors::from_fail(&f, &lim))
            }
            Err(f) => return Err(errors::fail_for(&f, &lim, r.op)),
        }
    }
    Err(last.unwrap_or_else(|| {
        ctx.tag(GatewayError::new(
            ErrorCode::NotConfigured,
            None,
            "Не задан адрес REST CoinEx",
        ))
    }))
}

/// Строка запроса: «имя=значение» через «&» в порядке параметров (без кодирования — см. шапку).
/// Ошибка Internal — в значении знак, который потребовал бы кодирования (строка в адресе и в подписи разошлась бы).
pub fn query(params: &Params) -> GatewayResult<String> {
    let mut out = Vec::with_capacity(params.len());
    for (k, v) in params {
        let safe = v
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.' | b','));
        if !safe {
            return Err(GatewayError::new(
                ErrorCode::Internal,
                None,
                format!("Недопустимый знак в параметре {k} запроса CoinEx: «{v}»"),
            ));
        }
        out.push(format!("{k}={v}"));
    }
    Ok(out.join("&"))
}

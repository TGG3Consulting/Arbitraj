//! Файл: venues/htx/rest.rs
//! Что делает: запросы REST к HTX — публичные и подписанные (sign.rs), разбор ответа (reply.rs), пауза после 429,
//! переход на резервный адрес только при сбое сети (подпись — заново, под host резервного адреса). Большой ответ
//! (справочники монет и сетей) разбирается вне рабочих потоков tokio (решение 131). Сжатие gzip — у общего HTTP-клиента
//! (решение 142, reqwest с gzip).
//! Кто вызывает: файлы адаптера HTX.
//! От чего зависит: ctx.rs, sign.rs, reply.rs, errors.rs, gateway/http.rs, reqwest.
//! Настройки: venue.rest_url, venue.rest_fallback_url [htx], http.*; секреты htx.*.
//!
//! Официально («Quick Start → Authentication»): у GET все параметры — в строке запроса и в подписи; у POST в строке
//! запроса только параметры подписи, тело — JSON («Content-Type: application/json»). Host подписи — host адреса запроса.

use reqwest::Method;
use serde_json::Value;

use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::Ctx;
use super::errors::{self, Op};
use super::{reply, sign};

/// Параметры строки запроса: пары ключ — значение.
pub type Params = Vec<(&'static str, String)>;

/// Один запрос.
pub struct Req<'a> {
    pub method: Method,
    /// Путь без строки запроса (например /v1/order/orders/place).
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

/// Публичный GET. Принимает: путь, параметры. Возвращает: data ответа. Ошибки: связь, коды HTX.
pub async fn public(ctx: &Ctx, path: &str, query: Params) -> GatewayResult<Value> {
    send(ctx, req(Method::GET, path, query, None, Op::Other, false)).await
}

/// Подписанный запрос. Принимает: метод, путь, параметры строки (у POST — пусто), тело, операцию.
/// Возвращает: data ответа. Ошибки: SecretsLocked/NotConfigured (ключи), коды HTX.
pub async fn signed(
    ctx: &Ctx,
    method: Method,
    path: &str,
    query: Params,
    body: Option<Value>,
    op: Op,
) -> GatewayResult<Value> {
    send(ctx, req(method, path, query, body, op, true)).await
}

/// Подписанный GET (кратко). Ошибки: как signed.
pub async fn get(ctx: &Ctx, path: &str, query: Params) -> GatewayResult<Value> {
    signed(ctx, Method::GET, path, query, None, Op::Other).await
}

/// Запрос без большого ответа. Ошибок нет.
pub fn req<'a>(
    method: Method,
    path: &'a str,
    query: Params,
    body: Option<Value>,
    op: Op,
    signed: bool,
) -> Req<'a> {
    Req {
        method,
        path,
        query,
        body,
        op,
        signed,
        heavy: false,
    }
}

/// Что делает: отправка запроса на адреса venue.rest_url (+ резерв только при сбое сети).
/// Принимает: окружение, запрос. Возвращает: data ответа. Ошибки: как public / signed.
pub async fn send(ctx: &Ctx, r: Req<'_>) -> GatewayResult<Value> {
    let urls = ctx.rest_urls()?;
    let body = r.body.as_ref().map(Value::to_string).unwrap_or_default();
    let lim = ctx.limits()?;
    // Пауза после 429 — запрос не отправляется вовсе.
    reply::gate().map_err(|e| ctx.tag(e))?;
    let mut last = None;
    for base in &urls {
        let q = query(ctx, &r, base)?;
        let url = if q.is_empty() {
            format!("{base}{}", r.path)
        } else {
            format!("{base}{}?{q}", r.path)
        };
        let mut req = ctx.client()?.request(r.method.clone(), url);
        if r.method == Method::POST {
            req = req
                .header("Content-Type", "application/json")
                .body(body.clone());
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
            "Не задан адрес REST HTX",
        ))
    }))
}

/// Строка запроса под адрес base: публичный — параметры; подписанный — с подписью (host — из base).
/// Ошибки: NotConfigured (адрес без host), ключи, InvalidCredentials.
fn query(ctx: &Ctx, r: &Req<'_>, base: &str) -> GatewayResult<String> {
    if !r.signed {
        return Ok(sign::encode(&r.query));
    }
    let (host, _) = sign::host_path(base).map_err(|m| {
        ctx.tag(GatewayError::not_set(
            None,
            "venue.rest_url",
            format!("HTX: {m}"),
        ))
    })?;
    let keys = ctx.keys()?;
    // У POST параметры строки не подписываются и не передаются (только тело JSON) — документация.
    let params: &[(&str, String)] = if r.method == Method::POST {
        &[]
    } else {
        &r.query
    };
    sign::signed_query(
        &keys,
        r.method.as_str(),
        &host,
        r.path,
        params,
        chrono::Utc::now(),
    )
    .map_err(|e| ctx.tag(e))
}

//! Файл: venues/poloniex/rest.rs
//! Что делает: запросы REST к Poloniex — публичные и подписанные (sign.rs), разбор ответа (reply.rs), пауза после 429,
//! переход на резервный адрес только при сбое сети. Большой ответ (справочник монет) разбирается вне рабочих потоков
//! tokio (решение 131). Сжатие gzip — у общего HTTP-клиента (решение 142, reqwest с gzip).
//! Кто вызывает: файлы адаптера Poloniex.
//! От чего зависит: ctx.rs, sign.rs, reply.rs, errors.rs, gateway/http.rs, reqwest.
//! Настройки: venue.rest_url, venue.rest_fallback_url [poloniex], http.*; секреты poloniex.*.
//!
//! Официально (Spot → API, https://api-docs.poloniex.com/spot/api/): REST «https://api.poloniex.com»; «POST and DELETE
//! requests must be sent in the form of application/json with a valid JSON body»; заголовки подписи key, signTimestamp,
//! signature. Строка запроса в адресе — та же, что в подписи (sign::sorted_query: по имени, значения кодированы как в
//! SDK), — у сервера и у нас одна строка.

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
    /// Путь без строки запроса (например /orders).
    pub path: &'a str,
    pub query: Params,
    /// Тело JSON (POST, DELETE).
    pub body: Option<Value>,
    pub op: Op,
    /// Подписанный запрос.
    pub signed: bool,
    /// Разбор ответа вне рабочих потоков (большой ответ).
    pub heavy: bool,
}

/// Публичный GET. Принимает: путь, параметры. Возвращает: тело ответа. Ошибки: связь, коды Poloniex.
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
    send(ctx, r).await
}

/// Подписанный запрос (кратко). Принимает: метод, путь, параметры строки, тело, операцию.
/// Возвращает: тело ответа. Ошибки: SecretsLocked/NotConfigured (ключи), коды Poloniex.
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
    send(ctx, r).await
}

/// Что делает: отправка запроса на адреса venue.rest_url (+ резерв только при сбое сети).
/// Принимает: окружение, запрос. Возвращает: тело ответа. Ошибки: как public / signed.
pub async fn send(ctx: &Ctx, r: Req<'_>) -> GatewayResult<Value> {
    let urls = ctx.rest_urls()?;
    let q = sign::sorted_query(&r.query);
    let path_q = if q.is_empty() {
        r.path.to_string()
    } else {
        format!("{}?{q}", r.path)
    };
    let body = r.body.as_ref().map(Value::to_string).unwrap_or_default();
    let lim = ctx.limits()?;
    // Пауза после 429 — запрос не отправляется вовсе.
    // Решение 201: ручная работа ждёт конец паузы (gateway/patience.rs), автоматика — отказ сразу.
    crate::gateway::patience::wait(reply::pause_left_ms()).await;
    reply::gate().map_err(|e| ctx.tag(e))?;
    let mut last = None;
    for base in &urls {
        let mut req = ctx
            .client()?
            .request(r.method.clone(), format!("{base}{path_q}"));
        if !body.is_empty() {
            req = req
                .header("Content-Type", "application/json")
                .body(body.clone());
        }
        if r.signed {
            let keys = ctx.keys()?;
            let ts = chrono::Utc::now().timestamp_millis();
            let plain = sign::payload(r.method.as_str(), r.path, &r.query, &body, ts);
            let signature =
                sign::hmac_b64(plain.as_bytes(), keys.secret.as_bytes()).map_err(|e| ctx.tag(e))?;
            req = req
                .header("key", keys.key.as_str())
                .header("signTimestamp", ts.to_string())
                .header("signature", signature);
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
            "Не задан адрес REST Poloniex",
        ))
    }))
}

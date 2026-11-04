//! Файл: venues/okx/rest.rs
//! Что делает: запросы REST к OKX — публичные и подписанные (sign.rs), разбор ответа (reply.rs), пауза пути после
//! «слишком часто», переход на резервный адрес только при сбое сети. Большой ответ (справочник монет) разбирается вне
//! рабочих потоков tokio (решение 131). Ответы сжатые (gzip) распаковывает HTTP-клиент (решение 142).
//! Кто вызывает: файлы адаптера OKX.
//! От чего зависит: ctx.rs, sign.rs, reply.rs, errors.rs, gateway/http.rs, reqwest.
//! Настройки: venue.rest_url, venue.rest_fallback_url [okx], http.*; секреты okx.*.
//!
//! Официально (https://www.okx.com/docs-v5/en/, «Overview → REST Authentication → Making Requests»): «All private REST
//! requests must contain the following headers: OK-ACCESS-KEY … OK-ACCESS-SIGN … OK-ACCESS-TIMESTAMP …
//! OK-ACCESS-PASSPHRASE»; «Request bodies should have content type application/json and be in valid JSON format»;
//! «GET request parameters are counted as requestpath, not body». Строка запроса подписывается в том же виде, в каком
//! уходит в адрес (SDK okx/utils.py parse_params_to_str — без кодирования; значения адаптера — буквы, цифры, «-», «,»).

use reqwest::Method;
use serde_json::Value;

use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::Ctx;
use super::errors::{self, Op};
use super::{reply, sign};

/// Параметры строки запроса: пары ключ — значение (порядок сохраняется — он же порядок подписи).
pub type Params = Vec<(&'static str, String)>;

/// Один запрос.
pub struct Req<'a> {
    pub method: Method,
    /// Путь без строки запроса (например /api/v5/trade/order).
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

/// Публичный GET. Принимает: путь, параметры. Возвращает: data ответа. Ошибки: связь, коды OKX.
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

/// Подписанный GET. Принимает: путь, параметры. Возвращает: data. Ошибки: SecretsLocked/NotConfigured, коды OKX.
pub async fn get(ctx: &Ctx, path: &str, query: Params) -> GatewayResult<Value> {
    let r = Req {
        method: Method::GET,
        path,
        query,
        body: None,
        op: Op::Other,
        signed: true,
        heavy: false,
    };
    send(ctx, r).await
}

/// Подписанный POST с телом JSON. Принимает: путь, тело, операцию. Возвращает: data. Ошибки: как get; у денег —
/// OutcomeUnknown, если ответ не получен.
pub async fn post(ctx: &Ctx, path: &str, body: Value, op: Op) -> GatewayResult<Value> {
    let r = Req {
        method: Method::POST,
        path,
        query: vec![],
        body: Some(body),
        op,
        signed: true,
        heavy: false,
    };
    send(ctx, r).await
}

/// Что делает: отправка запроса на адреса venue.rest_url (+ резерв только при сбое сети).
/// Принимает: окружение, запрос. Возвращает: data ответа. Ошибки: как public / get / post.
pub async fn send(ctx: &Ctx, r: Req<'_>) -> GatewayResult<Value> {
    let urls = ctx.rest_urls()?;
    let path_q = path_query(r.path, &r.query)?;
    let body = r.body.as_ref().map(Value::to_string).unwrap_or_default();
    let lim = ctx.limits()?;
    // Путь на паузе после «слишком часто» — запрос не отправляется вовсе.
    reply::gate(r.path).map_err(|e| ctx.tag(e))?;
    let mut last = None;
    for base in &urls {
        let mut req = ctx
            .http
            .request(r.method.clone(), format!("{base}{path_q}"))
            .header("Content-Type", "application/json");
        if r.signed {
            let keys = ctx.keys()?;
            let ts = sign::rest_timestamp(chrono::Utc::now());
            let h = sign::headers(&keys, &ts, r.method.as_str(), &path_q, &body)
                .map_err(|e| ctx.tag(e))?;
            req = req
                .header("OK-ACCESS-KEY", h.key)
                .header("OK-ACCESS-SIGN", h.sign)
                .header("OK-ACCESS-TIMESTAMP", h.timestamp)
                .header("OK-ACCESS-PASSPHRASE", h.passphrase);
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
            "Не задан адрес REST OKX (venue.rest_url)",
        ))
    }))
}

/// Путь со строкой запроса — один и тот же для адреса и подписи. Значения — только безопасные знаки (буквы, цифры,
/// «-_.,:»), иначе подпись разошлась бы с адресом после кодирования. Ошибка Internal — недопустимый знак.
pub fn path_query(path: &str, params: &Params) -> GatewayResult<String> {
    if params.is_empty() {
        return Ok(path.to_string());
    }
    let mut parts = Vec::with_capacity(params.len());
    for (k, v) in params {
        let safe = v
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b',' | b':'));
        if !safe {
            return Err(GatewayError::new(
                ErrorCode::Internal,
                None,
                format!("OKX: недопустимый знак в параметре {k} = «{v}»"),
            ));
        }
        parts.push(format!("{k}={v}"));
    }
    Ok(format!("{path}?{}", parts.join("&")))
}

/// Первая строка массива data (у OKX data — массив). Ошибка BadResponse — data не массив или пустой.
pub fn first<'a>(data: &'a Value, what: &str) -> GatewayResult<&'a Value> {
    data.as_array().and_then(|a| a.first()).ok_or_else(|| {
        errors::bad(
            &format!("{what}: data пустой или не массив"),
            &data.to_string(),
        )
    })
}

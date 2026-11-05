//! Файл: venues/bitget/rest.rs
//! Что делает: запросы REST к Bitget — публичные и подписанные (sign.rs), разбор ответа (reply.rs), пауза после 429,
//! переход на резервный адрес только при сбое сети. Большой ответ (справочник монет) разбирается вне рабочих потоков
//! tokio (решение 131). Сжатие gzip — у общего HTTP-клиента (решение 142, reqwest с gzip).
//! Кто вызывает: файлы адаптера Bitget.
//! От чего зависит: ctx.rs, sign.rs, reply.rs, errors.rs, gateway/http.rs, reqwest.
//! Настройки: venue.rest_url, venue.rest_fallback_url [bitget], http.*; секреты bitget.*.
//!
//! Официально (Classic → REST API, https://www.bitget.com/docs/classic/rest-api): параметры GET — в строке запроса,
//! подписывается «requestPath + "?" + queryString»; POST — тело JSON, «Content-Type: application/json»; заголовок
//! «locale» — язык ответа (zh-CN, en-US). Порядок и кодирование параметров в документации не описаны; официальный SDK
//! (bitget-python-sdk-api/bitget/utils.py: parse_params_to_str) сортирует параметры по имени и не кодирует значения —
//! так и здесь: параметры по имени, значения только из безопасных знаков (буквы, цифры, «_-.:»), тогда строка в адресе
//! и в подписи одна и та же.

use reqwest::Method;
use serde_json::Value;

use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::Ctx;
use super::errors::{self, Op};
use super::{reply, sign};

/// Параметры строки запроса: пары ключ — значение (отправляются отсортированными по имени).
pub type Params = Vec<(&'static str, String)>;

/// Язык ответов (заголовок locale) — одно из значений документации («zh-CN, en-US»).
const LOCALE: &str = "en-US";

/// Один запрос.
pub struct Req<'a> {
    pub method: Method,
    /// Путь без строки запроса (например /api/v2/spot/trade/place-order).
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

/// Публичный GET. Принимает: путь, параметры. Возвращает: data ответа. Ошибки: связь, коды Bitget.
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
/// Возвращает: data ответа. Ошибки: SecretsLocked/NotConfigured (ключи), коды Bitget.
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
/// Принимает: окружение, запрос. Возвращает: data ответа. Ошибки: как public / signed.
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
    // Пауза после 429 — запрос не отправляется вовсе.
    reply::gate().map_err(|e| ctx.tag(e))?;
    let mut last = None;
    for base in &urls {
        let mut req = ctx
            .client()?
            .request(r.method.clone(), format!("{base}{path_q}"))
            .header("locale", LOCALE);
        if r.method == Method::POST {
            req = req.header("Content-Type", "application/json");
        }
        if r.signed {
            let keys = ctx.keys()?;
            let ts = chrono::Utc::now().timestamp_millis();
            let h = sign::headers(&keys, ts, r.method.as_str(), &path_q, &body)
                .map_err(|e| ctx.tag(e))?;
            req = req
                .header("ACCESS-KEY", h.key)
                .header("ACCESS-SIGN", h.sign)
                .header("ACCESS-TIMESTAMP", h.timestamp)
                .header("ACCESS-PASSPHRASE", h.passphrase);
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
            "Не задан адрес REST Bitget",
        ))
    }))
}

/// Строка запроса: параметры по имени, «имя=значение» через «&» (без кодирования — см. шапку).
/// Ошибка Internal — в значении знак, который потребовал бы кодирования (строка в адресе и в подписи разошлась бы).
pub fn query(params: &Params) -> GatewayResult<String> {
    let mut sorted: Vec<&(&str, String)> = params.iter().collect();
    sorted.sort_by_key(|(k, _)| *k);
    let mut out = Vec::with_capacity(sorted.len());
    for (k, v) in sorted {
        let safe = v
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.' | b':'));
        if !safe {
            return Err(GatewayError::new(
                ErrorCode::Internal,
                None,
                format!("Недопустимый знак в параметре {k} запроса Bitget: «{v}»"),
            ));
        }
        out.push(format!("{k}={v}"));
    }
    Ok(out.join("&"))
}

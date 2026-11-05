//! Файл: venues/kraken/rest.rs
//! Что делает: запросы REST к Kraken — публичные (/0/public, GET), подписанные спот (/0/private, POST формой с nonce) и
//! подписанные Funding (Beta) (/funding/…, GET/POST/PUT, строка запроса и тело JSON); все подписанные — через одну
//! полосу ключа (ctx.lane: nonce и отправка под одним замком — ответа ждём до следующего запроса). Переход на резервный
//! адрес — только при сбое сети. Большой ответ разбирается вне рабочих потоков tokio (решение 131); gzip распаковывает
//! HTTP-клиент (решение 142).
//! Кто вызывает: файлы адаптера Kraken.
//! От чего зависит: ctx.rs, sign.rs, reply.rs, errors.rs, gateway/http.rs, reqwest.
//! Настройки: venue.rest_url, venue.rest_fallback_url [kraken], http.*; секреты kraken.*.
//!
//! Официально: [AUTH] https://docs.kraken.com/exchange/guides/rest/authentication.md — спот: заголовки API-Key, API-Sign,
//! nonce в теле POST (application/x-www-form-urlencoded), путь подписи /0/private/<метод>; [FUND]
//! https://docs.kraken.com/exchange/guides/rest/funding.md — Funding: api-key, api-sign, api-nonce, строка запроса —
//! urllib.parse.urlencode (вложенные ключи «asset[class]=…», скобки и значения кодируются), «the signed path includes
//! the query string», content-type application/json только при теле. Сервер Funding — https://api.kraken.com (без /0).

use reqwest::Method;
use serde_json::Value;

use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::Ctx;
use super::errors::{self, Op};
use super::{reply, sign};

/// Параметры: пары ключ — значение (порядок сохраняется — он же порядок подписи).
pub type Params = Vec<(String, String)>;

/// Пара параметра. Ошибок нет.
pub fn p(k: &str, v: impl Into<String>) -> (String, String) {
    (k.to_string(), v.into())
}

/// Кодирование как urllib.parse.urlencode (quote_plus): буквы, цифры и «_.-~» как есть, пробел — «+», прочее — %XX.
pub fn urlencode(params: &[(String, String)]) -> String {
    let enc = |s: &str| {
        let mut o = String::with_capacity(s.len());
        for b in s.bytes() {
            match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_' | b'.' | b'-' | b'~' => {
                    o.push(b as char)
                }
                b' ' => o.push('+'),
                _ => o.push_str(&format!("%{b:02X}")),
            }
        }
        o
    };
    params
        .iter()
        .map(|(k, v)| format!("{}={}", enc(k), enc(v)))
        .collect::<Vec<_>>()
        .join("&")
}

/// Публичный GET /0/public/<метод>. Принимает: метод («AssetPairs»), параметры. Возвращает: result.
/// Ошибки: связь, error[] Kraken.
pub async fn public(ctx: &Ctx, method: &str, query: Params) -> GatewayResult<Value> {
    let path = format!("/0/public/{method}");
    let q = urlencode(&query);
    let path_q = if q.is_empty() {
        path.clone()
    } else {
        format!("{path}?{q}")
    };
    reply::gate(&path).map_err(|e| ctx.tag(e))?;
    let lim = ctx.limits()?;
    let mut last = None;
    for base in ctx.rest_urls()? {
        let req = ctx.http.get(format!("{base}{path_q}"));
        match crate::gateway::http::exchange_full(req, lim).await {
            Ok(r) => return reply::parse_spot(ctx, &path, &path, &r, Op::Other),
            Err(f) if f.not_sent() || f.is_timeout() => last = Some(errors::from_fail(&f, &lim)),
            Err(f) => return Err(errors::from_fail(&f, &lim)),
        }
    }
    Err(last.unwrap_or_else(|| no_url(ctx)))
}

/// Подписанный POST /0/private/<метод> формой. Принимает: метод, параметры (без nonce), операцию, разбор вне рабочих
/// потоков. Возвращает: result. Ошибки: SecretsLocked/NotConfigured, error[] Kraken; у денег — OutcomeUnknown.
pub async fn private(ctx: &Ctx, method: &str, form: Params, op: Op) -> GatewayResult<Value> {
    let path = format!("/0/private/{method}");
    let key = if method == "AddOrder" {
        reply::TRADE
    } else {
        reply::PRIVATE
    };
    reply::gate(reply::PRIVATE).map_err(|e| ctx.tag(e))?;
    reply::gate(key).map_err(|e| ctx.tag(e))?;
    let keys = ctx.keys()?;
    sign::check(&keys).map_err(|e| ctx.tag(e))?;
    let lim = ctx.limits()?;
    let urls = ctx.rest_urls()?;
    let mut lane = ctx.lane.lock().await;
    let mut last = None;
    for base in &urls {
        *lane = sign::next_nonce(*lane, chrono::Utc::now().timestamp_millis().max(0) as u64);
        let nonce = lane.to_string();
        let mut all = vec![p("nonce", nonce.clone())];
        all.extend(form.iter().cloned());
        let body = urlencode(&all);
        let s =
            sign::api_sign(&path, &nonce, body.as_bytes(), &keys.secret).map_err(|e| ctx.tag(e))?;
        let req = ctx
            .http
            .post(format!("{base}{path}"))
            .header("API-Key", keys.key.as_str())
            .header("API-Sign", s)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(body);
        match crate::gateway::http::exchange_full(req, lim).await {
            Ok(r) => {
                let res = reply::parse_spot(ctx, &path, key, &r, op);
                return res.map_err(|e| errors::unsure(e, op));
            }
            Err(f) if f.not_sent() => last = Some(errors::from_fail(&f, &lim)),
            Err(f) => return Err(errors::fail_for(&f, &lim, op)),
        }
    }
    Err(last.unwrap_or_else(|| no_url(ctx)))
}

/// Подписанный запрос Funding (Beta). Принимает: HTTP-метод, путь (/funding/…), параметры строки запроса (вложенные —
/// уже «asset[class]»), тело JSON, операцию. Возвращает: тело ответа. Ошибки: как private; не 2xx — errors::from_funding.
pub async fn funding(
    ctx: &Ctx,
    method: Method,
    path: &str,
    query: Params,
    body: Option<Value>,
    op: Op,
) -> GatewayResult<Value> {
    reply::gate(reply::PRIVATE).map_err(|e| ctx.tag(e))?;
    let keys = ctx.keys()?;
    sign::check(&keys).map_err(|e| ctx.tag(e))?;
    let lim = ctx.limits()?;
    let urls = ctx.rest_urls()?;
    let q = urlencode(&query);
    let signed_path = if q.is_empty() {
        path.to_string()
    } else {
        format!("{path}?{q}")
    };
    // Компактный JSON — как json.dumps(separators=(",", ":")) помощника: serde_json пишет без пробелов.
    let body = body.as_ref().map(Value::to_string).unwrap_or_default();
    let mut lane = ctx.lane.lock().await;
    let mut last = None;
    for base in &urls {
        *lane = sign::next_nonce(*lane, chrono::Utc::now().timestamp_millis().max(0) as u64);
        let nonce = lane.to_string();
        let s = sign::api_sign(&signed_path, &nonce, body.as_bytes(), &keys.secret)
            .map_err(|e| ctx.tag(e))?;
        let mut req = ctx
            .http
            .request(method.clone(), format!("{base}{signed_path}"))
            .header("api-key", keys.key.as_str())
            .header("api-sign", s)
            .header("api-nonce", nonce);
        if !body.is_empty() {
            req = req
                .header("content-type", "application/json")
                .body(body.clone());
        }
        match crate::gateway::http::exchange_full(req, lim).await {
            Ok(r) => {
                let res = reply::parse_funding(ctx, path, &r, op);
                return res.map_err(|e| errors::unsure(e, op));
            }
            Err(f) if f.not_sent() => last = Some(errors::from_fail(&f, &lim)),
            Err(f) => return Err(errors::fail_for(&f, &lim, op)),
        }
    }
    Err(last.unwrap_or_else(|| no_url(ctx)))
}

/// Ошибка «не задан адрес REST».
fn no_url(ctx: &Ctx) -> GatewayError {
    ctx.tag(GatewayError::new(
        ErrorCode::NotConfigured,
        None,
        "Не задан адрес REST Kraken (venue.rest_url)",
    ))
}

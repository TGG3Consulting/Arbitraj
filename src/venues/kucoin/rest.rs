//! Файл: venues/kucoin/rest.rs
//! Что делает: запросы REST к KuCoin — публичные и подписанные (sign.rs), разбор ответа (reply.rs), пауза пула после
//! 429000, переход на резервный адрес только при сбое сети. Большой ответ (справочник монет) разбирается вне рабочих
//! потоков tokio (решение 131).
//! Кто вызывает: файлы адаптера KuCoin.
//! От чего зависит: ctx.rs, sign.rs, reply.rs, errors.rs, gateway/http.rs, reqwest.
//! Настройки: venue.rest_url, venue.rest_fallback_url, venue.api_key_version [kucoin], http.*; секреты kucoin.*.
//!
//! Официально (https://www.kucoin.com/docs-new/authentication): у GET/DELETE параметры — в строке запроса, тело
//! пустое; у POST — тело JSON; «Content-Type: application/json»; подписывается адрес до URL-кодирования (поэтому
//! строка для подписи — сырая, в адрес идёт закодированная). Ресурсный пул каждого эндпоинта — поле
//! x-api-rate-limit-pool спецификации SDK (github.com/Kucoin/kucoin-universal-sdk, spec/rest/api) — передаёт вызывающий.

use reqwest::Method;
use serde_json::Value;

use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::Ctx;
use super::errors::{self, Op};
use super::reply::{self, Pool};
use super::sign;

/// Параметры строки запроса: пары ключ — значение (порядок сохраняется — он же порядок подписи).
pub type Params = Vec<(&'static str, String)>;

/// Один запрос.
pub struct Req<'a> {
    pub method: Method,
    /// Путь без строки запроса (например /api/v1/hf/orders/sync).
    pub path: &'a str,
    pub query: Params,
    /// Тело JSON (только POST).
    pub body: Option<Value>,
    pub op: Op,
    pub pool: Pool,
    /// Подписанный запрос.
    pub signed: bool,
    /// Разбор ответа вне рабочих потоков (большой ответ).
    pub heavy: bool,
}

/// Публичный GET. Принимает: путь, параметры, пул. Возвращает: data ответа. Ошибки: связь, коды KuCoin.
pub async fn public(ctx: &Ctx, path: &str, query: Params, pool: Pool) -> GatewayResult<Value> {
    let r = Req {
        method: Method::GET,
        path,
        query,
        body: None,
        op: Op::Other,
        pool,
        signed: false,
        heavy: false,
    };
    send(ctx, r).await
}

/// Подписанный запрос (кратко). Принимает: метод, путь, параметры строки, тело, операцию, пул.
/// Возвращает: data ответа. Ошибки: SecretsLocked/NotConfigured (ключи, версия ключа), коды KuCoin.
pub async fn signed(
    ctx: &Ctx,
    method: Method,
    path: &str,
    query: Params,
    body: Option<Value>,
    op: Op,
    pool: Pool,
) -> GatewayResult<Value> {
    let r = Req {
        method,
        path,
        query,
        body,
        op,
        pool,
        signed: true,
        heavy: false,
    };
    send(ctx, r).await
}

/// Что делает: отправка запроса на адреса venue.rest_url (+ резерв при сбое сети).
/// Принимает: окружение, запрос. Возвращает: data ответа. Ошибки: как public / signed.
pub async fn send(ctx: &Ctx, r: Req<'_>) -> GatewayResult<Value> {
    let urls = ctx.rest_urls()?;
    send_at(ctx, &urls, r).await
}

/// Что делает: отправка запроса на заданные базовые адреса по порядку (следующий — только при сбое сети); нужен для
/// другого домена (фьючерсы — venue.futures_rest_url, all_balances.rs).
/// Принимает: окружение, базовые адреса без «/» в конце, запрос. Возвращает: data ответа. Ошибки: как public / signed.
pub async fn send_at(ctx: &Ctx, urls: &[String], r: Req<'_>) -> GatewayResult<Value> {
    let (raw_q, enc_q) = query(&r.query)?;
    let path_q = if raw_q.is_empty() {
        r.path.to_string()
    } else {
        format!("{}?{raw_q}", r.path)
    };
    let body = r.body.as_ref().map(Value::to_string).unwrap_or_default();
    let lim = ctx.limits()?;
    // Пул на паузе после 429000 — запрос не отправляется вовсе.
    reply::gate(r.pool).map_err(|e| ctx.tag(e))?;
    let mut last = None;
    for base in urls {
        let url = if enc_q.is_empty() {
            format!("{base}{}", r.path)
        } else {
            format!("{base}{}?{enc_q}", r.path)
        };
        let mut req = ctx
            .client()?
            .request(r.method.clone(), url)
            .header("Content-Type", "application/json");
        if r.signed {
            let keys = ctx.keys()?;
            let version = ctx.text("venue.api_key_version")?;
            let ts = chrono::Utc::now().timestamp_millis();
            let h = sign::headers(&keys, &version, ts, r.method.as_str(), &path_q, &body)
                .map_err(|e| ctx.tag(e))?;
            req = req
                .header("KC-API-KEY", h.key)
                .header("KC-API-SIGN", h.sign)
                .header("KC-API-TIMESTAMP", h.timestamp)
                .header("KC-API-PASSPHRASE", h.passphrase)
                .header("KC-API-KEY-VERSION", h.version);
        }
        if !body.is_empty() {
            req = req.body(body.clone());
        }
        match crate::gateway::http::exchange_full(req, lim).await {
            Ok(reply) => {
                let parsed = if r.heavy {
                    let (c, p, op, pool) = (ctx.clone(), r.path.to_string(), r.op, r.pool);
                    tokio::task::spawn_blocking(move || reply::parse(&c, &p, &reply, op, pool))
                        .await
                        .map_err(|e| {
                            ctx.tag(GatewayError::new(
                                ErrorCode::Internal,
                                None,
                                format!("разбор ответа {} прерван: {e}", r.path),
                            ))
                        })?
                } else {
                    reply::parse(ctx, r.path, &reply, r.op, r.pool)
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
            "Не задан адрес REST KuCoin",
        ))
    }))
}

/// Строка запроса: (сырая — для подписи, закодированная — для адреса). Ошибка Internal — перевод строки в значении.
fn query(params: &Params) -> GatewayResult<(String, String)> {
    let (mut raw, mut enc) = (Vec::new(), Vec::new());
    for (k, v) in params {
        if v.contains(['\r', '\n']) {
            return Err(GatewayError::new(
                ErrorCode::Internal,
                None,
                format!("Недопустимый символ в параметре {k}"),
            ));
        }
        raw.push(format!("{k}={v}"));
        enc.push(format!("{k}={}", encode(v)));
    }
    Ok((raw.join("&"), enc.join("&")))
}

/// Кодирование значения: буквы, цифры и «-_.~» как есть, остальное — %XX. Ошибок нет.
fn encode(v: &str) -> String {
    let mut s = String::with_capacity(v.len());
    for b in v.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            s.push(b as char);
        } else {
            s.push_str(&format!("%{b:02X}"));
        }
    }
    s
}

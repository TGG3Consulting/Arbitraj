//! Файл: venues/bybit/rest.rs
//! Что делает: запросы REST к Bybit V5 — публичные и подписанные (HMAC SHA256), разбор ответа, перевод ошибок в коды
//! шлюза, паузы по лимитам (reply.rs), переход на резервный адрес только при сбое сети. Большой ответ (signed_heavy)
//! разбирается вне рабочих потоков tokio.
//! Кто вызывает: файлы адаптера Bybit.
//! От чего зависит: ctx.rs, errors.rs, reply.rs, http.rs, hmac, sha2, reqwest.
//! Настройки: venue.rest_url, venue.rest_fallback_url, venue.recv_window_ms, http.*; секреты bybit.api_key,
//! bybit.api_secret.
//!
//! Подпись — по официальному разделу «Authentication» https://bybit-exchange.github.io/docs/v5/guide: заголовки
//! X-BAPI-API-KEY, X-BAPI-TIMESTAMP («UTC timestamp in milliseconds»), X-BAPI-SIGN, X-BAPI-RECV-WINDOW; строка подписи
//! «GET: timestamp + API key + recv_window + queryString», «POST: timestamp + API key + recv_window + jsonBodyString»;
//! HMAC_SHA256 → «lowercase HEX». POST — Content-Type application/json. Время запроса должно лежать в
//! «[server_time - recv_window; server_time + 1000)» — окно задаёт venue.recv_window_ms.

use hmac::{Hmac, KeyInit, Mac};
use reqwest::Method;
use serde_json::Value;
use sha2::Sha256;

use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::Ctx;
use super::errors::{self, Op};
use super::reply;

/// Параметры строки запроса: пары ключ — значение (порядок сохраняется — он же порядок подписи).
pub type Params = Vec<(&'static str, String)>;

/// Публичный GET (без подписи). Возвращает: поле result. Ошибки: коды Bybit, связь.
pub async fn public(ctx: &Ctx, path: &str, params: Params) -> GatewayResult<Value> {
    let q = query(&params)?;
    send(ctx, Method::GET, path, &q, None, None, Op::Other).await
}

/// Подписанный GET. Принимает: путь, параметры строки запроса, операцию. Возвращает: result.
/// Ошибки: SecretsLocked/NotConfigured (ключи, окно), коды Bybit, связь.
pub async fn get(ctx: &Ctx, path: &str, params: Params, op: Op) -> GatewayResult<Value> {
    let q = query(&params)?;
    let auth = sign(ctx, &q)?;
    send(ctx, Method::GET, path, &q, None, Some(auth), op).await
}

/// Подписанный POST с телом JSON. Принимает: путь, тело (объект), операцию. Возвращает: result.
/// Ошибки: как get.
pub async fn post(ctx: &Ctx, path: &str, body: &Value, op: Op) -> GatewayResult<Value> {
    let text = body.to_string();
    let auth = sign(ctx, &text)?;
    send(ctx, Method::POST, path, "", Some(text), Some(auth), op).await
}

/// Подписанный GET с большим ответом (справочник монет): разбор JSON — в потоке блокирующих задач.
pub async fn get_heavy(ctx: &Ctx, path: &str, params: Params, op: Op) -> GatewayResult<Value> {
    HEAVY.scope(true, get(ctx, path, params, op)).await
}

tokio::task_local! {
    /// Разбор ответа — вне рабочих потоков (get_heavy).
    static HEAVY: bool;
}

/// Заголовки подписи: ключ, время, окно, подпись.
struct Auth {
    key: String,
    ts: String,
    recv: String,
    sign: String,
}

/// Что делает: подпись запроса по строке «timestamp + key + recv_window + payload».
/// Принимает: окружение, payload (строка запроса или тело JSON). Возвращает: заголовки.
/// Ошибки: SecretsLocked, NotConfigured (ключи, venue.recv_window_ms), InvalidCredentials (пустой секрет).
fn sign(ctx: &Ctx, payload: &str) -> GatewayResult<Auth> {
    let (key, secret) = ctx.keys()?;
    let recv = ctx.int("venue.recv_window_ms")?.to_string();
    let ts = chrono::Utc::now().timestamp_millis().to_string();
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).map_err(|_| {
        ctx.tag(GatewayError::new(
            ErrorCode::InvalidCredentials,
            None,
            "Пустой секретный ключ Bybit",
        ))
    })?;
    mac.update(format!("{ts}{}{recv}{payload}", key.as_str()).as_bytes());
    Ok(Auth {
        key: key.to_string(),
        ts,
        recv,
        sign: hex::encode(mac.finalize().into_bytes()),
    })
}

/// Строка параметров «k=v&k=v» в порядке передачи (он же порядок подписи); значения кодируются для URL.
/// Ошибка Internal — перевод строки в значении (защита от подмены параметров).
fn query(params: &Params) -> GatewayResult<String> {
    let mut out = Vec::with_capacity(params.len());
    for (k, v) in params {
        if v.contains(['\r', '\n']) {
            return Err(GatewayError::new(
                ErrorCode::Internal,
                None,
                format!("Недопустимый символ в параметре {k}"),
            ));
        }
        out.push(format!("{k}={}", encode(v)));
    }
    Ok(out.join("&"))
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

/// Отправка с переходом на резервный адрес при сбое сети. Перед запросом — пауза по лимитам (reply::gate).
async fn send(
    ctx: &Ctx,
    m: Method,
    path: &str,
    q: &str,
    body: Option<String>,
    auth: Option<Auth>,
    op: Op,
) -> GatewayResult<Value> {
    let urls = ctx.rest_urls()?;
    let lim = ctx.limits()?;
    // Решение 201: ручная работа ждёт конец паузы (gateway/patience.rs).
    crate::gateway::patience::wait(reply::pause_left_ms(path)).await;

    reply::gate(path)?;
    let mut last = None;
    for base in &urls {
        let url = if q.is_empty() {
            format!("{base}{path}")
        } else {
            format!("{base}{path}?{q}")
        };
        let mut req = ctx.client()?.request(m.clone(), url);
        if let Some(a) = &auth {
            req = req
                .header("X-BAPI-API-KEY", &a.key)
                .header("X-BAPI-TIMESTAMP", &a.ts)
                .header("X-BAPI-RECV-WINDOW", &a.recv)
                .header("X-BAPI-SIGN", &a.sign);
        }
        if let Some(b) = &body {
            req = req
                .header("Content-Type", "application/json")
                .body(b.clone());
        }
        match super::http::exchange(req, lim).await {
            Ok(r) => {
                let parsed = if HEAVY.try_with(|h| *h).unwrap_or(false) {
                    let (c, p) = (ctx.clone(), path.to_string());
                    tokio::task::spawn_blocking(move || reply::parse(&c, &p, &r, op))
                        .await
                        .map_err(|e| {
                            ctx.tag(GatewayError::new(
                                ErrorCode::Internal,
                                None,
                                format!("разбор ответа {path} прерван: {e}"),
                            ))
                        })?
                } else {
                    reply::parse(ctx, path, &r, op)
                };
                return parsed.map_err(|e| errors::unsure(e, op));
            }
            // Резерв — только если запрос точно не дошёл; иначе ордер или вывод мог исполниться — не повторяем.
            Err(f) if may_fallback(&f, op) => last = Some(errors::from_fail(&f, &lim)),
            Err(f) => return Err(errors::fail_for(&f, &lim, op)),
        }
    }
    Err(last.unwrap_or_else(|| {
        ctx.tag(GatewayError::new(
            ErrorCode::NotConfigured,
            None,
            "Не задан адрес REST",
        ))
    }))
}

/// Можно ли повторить на резервном адресе: запрос точно не дошёл, или срок без ответа — только у справочников.
pub fn may_fallback(f: &crate::gateway::http::Fail, op: Op) -> bool {
    f.not_sent() || (op == Op::Other && f.is_timeout())
}

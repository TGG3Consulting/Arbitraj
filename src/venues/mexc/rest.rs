//! Файл: venues/mexc/rest.rs
//! Что делает: запросы REST к MEXC — публичные и подписанные (HMAC SHA256), разбор ответа, перевод ошибок в коды
//! шлюза, переход на резервный адрес только при сбое сети. Ответы пишутся в сырой журнал (raw.*), кроме пути listenKey.
//! Кто вызывает: файлы адаптера MEXC.
//! От чего зависит: ctx.rs, errors.rs, hmac, sha2, reqwest.
//! Настройки: venue.rest_url, venue.rest_fallback_url, venue.recv_window_ms, http.*; секреты mexc.api_key, mexc.api_secret.
//!
//! Подпись — по официальному разделу «Signature» https://www.mexc.com/api-docs/spot-v3/introduction:
//! «Signatures use the HMAC SHA256 algorithm», «totalParams = queryString + requestBody», «the parameter sequence used
//! when generating the signature must match the sequence actually sent», «lowercase hexadecimal only», заголовок ключа
//! `X-MEXC-APIKEY`. Все параметры (в том числе у POST/PUT/DELETE) отправляются в строке запроса — это разрешено
//! («parameters may be sent entirely in the Query String») и снимает вопрос Content-Type (application/x-www-form-urlencoded
//! даёт 700013). recvWindow — до 60000 («The maximum is 60 seconds»).

use hmac::{Hmac, KeyInit, Mac};
use reqwest::Method;
use serde_json::Value;
use sha2::Sha256;

use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::Ctx;
use super::errors::{self, Op};

/// Предел recvWindow — из официальной документации («The maximum is 60 seconds»).
const MAX_RECV_WINDOW_MS: u64 = 60_000;

/// Параметры запроса: пары ключ — значение (порядок сохраняется — он же порядок подписи).
pub type Params = Vec<(&'static str, String)>;

/// Публичный GET (без подписи). Возвращает: ответ (поле data, если ответ в обёртке {code, data}).
pub async fn public(ctx: &Ctx, path: &str, params: Params) -> GatewayResult<Value> {
    let query = query(&params)?;
    send(ctx, Method::GET, path, &query, None, Op::Other).await
}

/// Подписанный запрос. Принимает: метод, путь, параметры (без timestamp/recvWindow), вид операции.
/// Возвращает: ответ. Ошибки: SecretsLocked/NotConfigured (ключи), единые коды ошибок MEXC.
pub async fn signed(
    ctx: &Ctx,
    method: Method,
    path: &str,
    mut params: Params,
    op: Op,
) -> GatewayResult<Value> {
    let (key, secret) = ctx.keys()?;
    let recv = ctx.int("venue.recv_window_ms")?;
    if recv > MAX_RECV_WINDOW_MS {
        return Err(ctx.tag(GatewayError::new(
            ErrorCode::NotConfigured,
            None,
            format!("venue.recv_window_ms = {recv}: у MEXC не больше {MAX_RECV_WINDOW_MS}"),
        )));
    }
    params.push(("recvWindow", recv.to_string()));
    params.push((
        "timestamp",
        chrono::Utc::now().timestamp_millis().to_string(),
    ));
    let canon = query(&params)?;
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).map_err(|_| {
        ctx.tag(GatewayError::new(
            ErrorCode::InvalidCredentials,
            None,
            "Пустой секретный ключ MEXC",
        ))
    })?;
    mac.update(canon.as_bytes());
    let signature = hex::encode(mac.finalize().into_bytes());
    let full = format!("{canon}&signature={signature}");
    send(ctx, method, path, &full, Some(key.as_str()), op).await
}

/// Строка параметров «k=v&k=v» в порядке передачи (он же порядок подписи). Значения кодируются по правилам строки
/// запроса (запятые в списках — «URL-encode them when signing; encoding must use uppercase only»).
/// Ошибка Internal — в значении перевод строки (защита от подмены параметров).
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

/// Кодирование значения для строки запроса: буквы, цифры и «-_.~» как есть, остальное — %XX в верхнем регистре.
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

/// Отправка с переходом на резервный адрес при сбое сети. key — заголовок X-MEXC-APIKEY (None — публичный запрос).
async fn send(
    ctx: &Ctx,
    m: Method,
    path: &str,
    query: &str,
    key: Option<&str>,
    op: Op,
) -> GatewayResult<Value> {
    let urls = ctx.rest_urls()?;
    let lim = ctx.limits()?;
    let mut last = None;
    for base in &urls {
        let url = if query.is_empty() {
            format!("{base}{path}")
        } else {
            format!("{base}{path}?{query}")
        };
        let mut req = ctx.http.request(m.clone(), url);
        if let Some(key) = key {
            req = req.header("X-MEXC-APIKEY", key);
        }
        match crate::gateway::http::exchange(req, lim).await {
            Ok((status, text)) => {
                return parse(ctx, path, status.as_u16(), text, op)
                    .map_err(|e| errors::unsure(e, op));
            }
            // Резерв — только если запрос точно не дошёл; иначе ордер или вывод мог исполниться — не повторяем.
            Err(f) if may_fallback(&f, op) => last = Some(errors::from_fail(&f, &lim)),
            Err(f) => return Err(errors::fail_for(&f, &lim, op)),
        }
    }
    Err(last.unwrap_or_else(|| {
        GatewayError::new(
            ErrorCode::NotConfigured,
            Some(ctx.venue()),
            "Не задан адрес REST",
        )
    }))
}

/// Разбор ответа. Общей схемы у MEXC нет (документация): успех — объект или массив без поля code (account, order,
/// depth, getall) либо обёртка {code: 0|200, data, msg} (tradeFee, selfSymbols, defaultSymbols); ошибка — {code, msg}
/// с HTTP 4XX или кодом не 0/200. Текст ответа — в сырой журнал (кроме пути listenKey: ключ — секрет сессии).
fn parse(ctx: &Ctx, path: &str, status: u16, text: String, op: Op) -> GatewayResult<Value> {
    if !path.contains(super::user_key::PATH) {
        ctx.raw
            .write(super::ctx::ID, &format!("rest {path}"), &text);
    }
    let Ok(v) = serde_json::from_str::<Value>(&text) else {
        return Err(errors::from_http(status, &text));
    };
    let code = v.get("code").and_then(Value::as_i64);
    match code {
        Some(0) | Some(200) if status < 400 => Ok(v.get("data").cloned().unwrap_or(v)),
        Some(c) if v.get("msg").is_some() => {
            let msg = v.get("msg").and_then(Value::as_str).unwrap_or_default();
            Err(errors::from_code(c, msg, op, &text))
        }
        _ if status < 400 => Ok(v),
        _ => Err(errors::from_http(status, &text)),
    }
}

/// Можно ли повторить запрос на резервном адресе: только если запрос точно не дошёл (соединение не установлено), а
/// срок без ответа — только для справочников (Op::Other). Принимает: сбой, операцию. Ошибок нет.
pub fn may_fallback(f: &crate::gateway::http::Fail, op: Op) -> bool {
    f.not_sent() || (op == Op::Other && f.is_timeout())
}

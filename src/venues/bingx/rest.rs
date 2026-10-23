//! Файл: venues/bingx/rest.rs
//! Что делает: запросы REST к BingX — публичные и подписанные (HMAC SHA256), разбор ответа
//! {code, msg, data}, перевод ошибок в коды шлюза, переход на резервный адрес только при сбое сети.
//! Кто вызывает: файлы адаптера BingX (symbols, wallet, transfers, account, trade, order_state, fees).
//! От чего зависит: ctx.rs, errors.rs, hmac, sha2, reqwest.
//! Настройки: venue.rest_url, venue.rest_fallback_url, venue.recv_window_ms, http.timeout_ms;
//! секреты bingx.api_key, bingx.api_secret.
//!
//! Подпись — по официальной документации «Authentication»:
//! https://github.com/BingX-API/api-ai-skills/blob/main/skills/references/authentication.md
//! все параметры (с timestamp) сортируются по ключу (ASCII), склеиваются «k=v&k=v» без URL-кодирования,
//! HMAC SHA256 секретом, hex в нижнем регистре → параметр signature; ключ — заголовок X-BX-APIKEY.
//! GET/DELETE/PUT — параметры в строке запроса; POST — тело application/x-www-form-urlencoded.
//! Резервный адрес — только при сбое сети или таймауте, никогда при ошибке бизнес-логики
//! (https://github.com/BingX-API/api-ai-skills/blob/main/skills/references/base-urls.md).

use hmac::{Hmac, KeyInit, Mac};
use reqwest::Method;
use serde_json::Value;
use sha2::Sha256;

use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::Ctx;
use super::errors::{self, Op};

/// Предел recvWindow — из официальной документации (authentication.md: «maximum is 5000»).
const MAX_RECV_WINDOW_MS: u64 = 5000;

/// Параметры запроса: пары ключ — значение.
pub type Params = Vec<(&'static str, String)>;

/// Публичный GET (без подписи). Возвращает: поле data (или весь ответ, если data нет).
pub async fn public(ctx: &Ctx, path: &str, params: Params) -> GatewayResult<Value> {
    let query = canonical(&params)?;
    send(ctx, Method::GET, path, &query, None, Op::Other).await
}

/// Подписанный запрос. Принимает: метод, путь, параметры (без timestamp/recvWindow), вид операции.
/// Возвращает: поле data (или весь ответ, если data нет — например, голый массив истории).
/// Ошибки: SecretsLocked/NotConfigured (ключи), единые коды ошибок BingX.
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
            format!("venue.recv_window_ms = {recv}: у BingX не больше {MAX_RECV_WINDOW_MS}"),
        )));
    }
    params.push(("recvWindow", recv.to_string()));
    params.push((
        "timestamp",
        chrono::Utc::now().timestamp_millis().to_string(),
    ));
    let canon = canonical(&params)?;
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).map_err(|_| {
        ctx.tag(GatewayError::new(
            ErrorCode::InvalidCredentials,
            None,
            "Пустой секретный ключ BingX",
        ))
    })?;
    mac.update(canon.as_bytes());
    let signature = hex::encode(mac.finalize().into_bytes());
    let full = format!("{canon}&signature={signature}");
    if method == Method::POST {
        send(ctx, method, path, "", Some((key.as_str(), full)), op).await
    } else {
        send_with_key(ctx, method, path, &full, key.as_str(), op).await
    }
}

/// Строка параметров: сортировка по ключу (ASCII), «k=v&k=v», без кодирования.
/// Ошибка Internal — в значении есть &, =, ?, # или перевод строки (защита от подмены параметров,
/// как в официальном validateParams).
fn canonical(params: &Params) -> GatewayResult<String> {
    let mut sorted: Vec<&(&str, String)> = params.iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(b.0));
    for (k, v) in &sorted {
        if v.contains(['&', '=', '?', '#', '\r', '\n']) {
            return Err(GatewayError::new(
                ErrorCode::Internal,
                None,
                format!("Недопустимый символ в параметре {k}"),
            ));
        }
    }
    Ok(sorted
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("&"))
}

/// GET/PUT/DELETE с ключом в заголовке.
async fn send_with_key(
    ctx: &Ctx,
    m: Method,
    path: &str,
    query: &str,
    key: &str,
    op: Op,
) -> GatewayResult<Value> {
    send(ctx, m, path, query, Some((key, String::new())), op).await
}

/// Отправка с переходом на резервный адрес при сбое сети.
/// auth: (ключ, тело POST); тело пустое — параметры в строке запроса.
async fn send(
    ctx: &Ctx,
    m: Method,
    path: &str,
    query: &str,
    auth: Option<(&str, String)>,
    op: Op,
) -> GatewayResult<Value> {
    let urls = ctx.rest_urls()?;
    let timeout = ctx.http_timeout()?;
    let mut last = None;
    for base in &urls {
        let url = if query.is_empty() {
            format!("{base}{path}")
        } else {
            format!("{base}{path}?{query}")
        };
        let mut req = ctx.http.request(m.clone(), url).timeout(timeout);
        if let Some((key, body)) = &auth {
            req = req.header("X-BX-APIKEY", *key);
            if !body.is_empty() {
                req = req
                    .header("Content-Type", "application/x-www-form-urlencoded")
                    .body(body.clone());
            }
        }
        match req.send().await {
            Ok(resp) => return parse(resp, op).await,
            // Резерв — только если запрос точно не дошёл до BingX (нет соединения).
            // Таймаут ордера или вывода не повторяем: он мог исполниться — повтор = двойная сделка.
            Err(e) if e.is_connect() || (op == Op::Other && e.is_timeout()) => {
                last = Some(errors::from_network(e))
            }
            Err(e) => return Err(errors::from_network(e)),
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

/// Разбор ответа: {code, msg, data} или голый массив (истории ввода и вывода).
async fn parse(resp: reqwest::Response, op: Op) -> GatewayResult<Value> {
    let status = resp.status().as_u16();
    let text = resp.text().await.map_err(errors::from_network)?;
    let Ok(v) = serde_json::from_str::<Value>(&text) else {
        return Err(errors::from_http(status, &text));
    };
    if v.is_array() {
        return Ok(v);
    }
    match v.get("code").and_then(Value::as_i64) {
        Some(0) => Ok(v.get("data").cloned().unwrap_or(v)),
        Some(code) => {
            let msg = v.get("msg").and_then(Value::as_str).unwrap_or_default();
            Err(errors::from_code(code, msg, op, &text))
        }
        // Без поля code: успешный ответ отдаётся целиком (например {"listenKey": …} или {"transferId": …}).
        None if status < 400 => Ok(v),
        None => Err(errors::from_http(status, &text)),
    }
}

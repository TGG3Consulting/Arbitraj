//! Файл: venues/bingx/rest.rs
//! Что делает: запросы REST к BingX — публичные и подписанные (HMAC SHA256), разбор ответа
//! {code, msg, data}, перевод ошибок в коды шлюза, переход на резервный адрес только при сбое сети.
//! Ответы REST пишутся в сырой журнал (raw.*, как сообщения WebSocket), кроме путей listenKey.
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
    let lim = ctx.limits()?;
    let mut last = None;
    for base in &urls {
        let url = if query.is_empty() {
            format!("{base}{path}")
        } else {
            format!("{base}{path}?{query}")
        };
        let mut req = ctx.http.request(m.clone(), url);
        if let Some((key, body)) = &auth {
            req = req.header("X-BX-APIKEY", *key);
            if !body.is_empty() {
                req = req
                    .header("Content-Type", "application/x-www-form-urlencoded")
                    .body(body.clone());
            }
        }
        // Два срока (gateway/http.rs): до заголовков — http.timeout_ms, тело — пока данные идут чаще
        // http.body_idle_ms (справочник пар и сетей BingX — большой ответ).
        match crate::gateway::http::exchange(req, lim).await {
            // Ордер или вывод без ясного итога — «результат неизвестен» (errors::unsure, решение 64).
            Ok((status, text)) => {
                return parse(ctx, path, status.as_u16(), text, op)
                    .map_err(|e| errors::unsure(e, op));
            }
            // Резерв — только если запрос точно не дошёл до BingX (нет соединения).
            // Иной сбой ордера или вывода — «результат неизвестен» (errors::fail_for): мог исполниться, не повторяем.
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

/// Разбор ответа: {code, msg, data} или голый массив (истории ввода и вывода). Текст ответа — в сырой журнал
/// (data/raw/bingx, соединение «rest <путь>»), кроме путей listenKey (ключ потока аккаунта — секрет сессии).
fn parse(ctx: &Ctx, path: &str, status: u16, text: String, op: Op) -> GatewayResult<Value> {
    if !path.contains("listenKey") {
        ctx.raw
            .write(super::ctx::ID, &format!("rest {path}"), &text);
    }
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

/// Можно ли повторить запрос на резервном адресе: только если запрос точно не дошёл до BingX (соединение не
/// установлено), а срок без ответа — только для справочников и проверок (Op::Other). Ордер или вывод без ответа мог
/// исполниться — повтор = двойная сделка. Принимает: сбой, операцию. Ошибок нет.
pub fn may_fallback(f: &crate::gateway::http::Fail, op: Op) -> bool {
    f.not_sent() || (op == Op::Other && f.is_timeout())
}

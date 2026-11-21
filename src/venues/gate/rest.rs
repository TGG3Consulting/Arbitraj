//! Файл: venues/gate/rest.rs
//! Что делает: запросы REST к Gate — публичные и подписанные (HMAC-SHA512), разбор ответа, перевод ошибок в коды
//! шлюза, переход на резервный адрес только при сбое сети. Большой ответ (heavy) разбирается вне рабочих потоков tokio.
//! Кто вызывает: файлы адаптера Gate.
//! От чего зависит: ctx.rs, errors.rs, reply.rs, gateway/http.rs, hmac, sha2, reqwest.
//! Настройки: venue.rest_url, venue.rest_fallback_url, http.*; секреты gate.api_key, gate.api_secret.
//!
//! Подпись — по официальному разделу «Authentication» API v4 https://www.gate.com/docs/developers/apiv4/en/ и коду
//! официального SDK (gate_api/api_client.py, gen_sign: https://github.com/gate/gateapi-python): заголовки KEY (ключ),
//! Timestamp (Unix, секунды; расхождение с сервером — не больше 60 с), SIGN — hex(HMAC-SHA512(секрет, строка)), где
//! строка = «Метод\nПуть\nСтрока запроса\nhex(SHA512(тело))\nTimestamp»; путь — путь URL целиком с префиксом
//! (/api/v4/spot/orders), строка запроса — без URL-кодирования (SDK: unquote_plus(urlencode(…))), пустое тело — SHA512
//! пустой строки. Тело POST — JSON (Content-Type: application/json — «HTTP request headers» методов SDK).

use hmac::{Hmac, KeyInit, Mac};
use reqwest::Method;
use serde_json::Value;
use sha2::{Digest, Sha512};

use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::Ctx;
use super::errors::{self, Op};
use super::reply;

/// Параметры строки запроса: пары ключ — значение (порядок сохраняется — он же порядок подписи).
pub type Params = Vec<(&'static str, String)>;

/// Публичный GET (без подписи). Возвращает: тело ответа. Ошибки: связь, коды Gate.
pub async fn public(ctx: &Ctx, path: &str, params: Params) -> GatewayResult<Value> {
    send(ctx, Method::GET, path, &params, None, false, Op::Other).await
}

/// Публичный GET с большим ответом (решение 131: весь справочник монет): разбор — вне рабочих потоков tokio.
pub async fn public_heavy(ctx: &Ctx, path: &str, params: Params) -> GatewayResult<Value> {
    HEAVY.scope(true, public(ctx, path, params)).await
}

/// Подписанный запрос. Принимает: метод, путь (от адреса REST, например /spot/orders), строку запроса, тело JSON,
/// вид операции. Возвращает: тело ответа. Ошибки: SecretsLocked/NotConfigured (ключи), коды Gate.
pub async fn signed(
    ctx: &Ctx,
    method: Method,
    path: &str,
    params: Params,
    body: Option<Value>,
    op: Op,
) -> GatewayResult<Value> {
    send(ctx, method, path, &params, body.as_ref(), true, op).await
}

/// Подписанный GET с большим ответом (весь список комиссий вывода): разбор — вне рабочих потоков tokio.
pub async fn signed_heavy(ctx: &Ctx, path: &str, params: Params) -> GatewayResult<Value> {
    HEAVY
        .scope(
            true,
            signed(ctx, Method::GET, path, params, None, Op::Other),
        )
        .await
}

tokio::task_local! {
    /// Разбор ответа — вне рабочих потоков (public_heavy, signed_heavy).
    static HEAVY: bool;
}

/// Что делает: строка параметров «k=v&k=v» — без кодирования (для подписи) или с кодированием (для отправки).
/// Ошибка Internal — в значении перевод строки (подмена строки подписи).
fn query(params: &Params, encoded: bool) -> GatewayResult<String> {
    let mut out = Vec::with_capacity(params.len());
    for (k, v) in params {
        if v.contains(['\r', '\n']) {
            let m = format!("Недопустимый символ в параметре {k}");
            return Err(GatewayError::new(ErrorCode::Internal, None, m));
        }
        let v = if encoded { encode(v) } else { v.clone() };
        out.push(format!("{k}={v}"));
    }
    Ok(out.join("&"))
}

/// Кодирование значения строки запроса: буквы, цифры и «-_.~» как есть, остальное — %XX. Ошибок нет.
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

/// Что делает: заголовки подписи (KEY, Timestamp, SIGN) по шапке файла.
/// Принимает: окружение, метод, полный путь URL, строку запроса без кодирования, тело. Ошибки: ключи.
fn sign(
    ctx: &Ctx,
    m: &Method,
    full_path: &str,
    q: &str,
    body: &str,
) -> GatewayResult<[(&'static str, String); 3]> {
    let (key, secret) = ctx.keys()?;
    let ts = chrono::Utc::now().timestamp().to_string();
    let hashed = hex::encode(Sha512::digest(body.as_bytes()));
    let text = format!("{}\n{full_path}\n{q}\n{hashed}\n{ts}", m.as_str());
    let mut mac = Hmac::<Sha512>::new_from_slice(secret.as_bytes()).map_err(|_| {
        ctx.tag(GatewayError::new(
            ErrorCode::InvalidCredentials,
            None,
            "Пустой секретный ключ Gate",
        ))
    })?;
    mac.update(text.as_bytes());
    Ok([
        ("KEY", key.to_string()),
        ("Timestamp", ts),
        ("SIGN", hex::encode(mac.finalize().into_bytes())),
    ])
}

/// Что делает: отправка с переходом на резервный адрес при сбое сети (только если запрос точно не дошёл).
/// Ошибки: пауза после 429, настройки, связь, коды Gate.
async fn send(
    ctx: &Ctx,
    m: Method,
    path: &str,
    params: &Params,
    body: Option<&Value>,
    auth: bool,
    op: Op,
) -> GatewayResult<Value> {
    let urls = ctx.rest_urls()?;
    let lim = ctx.limits()?;
    // Решение 201: ручная работа ждёт конец паузы (gateway/patience.rs), автоматика — отказ сразу.
    crate::gateway::patience::wait(reply::pause_left_ms()).await;
    reply::gate()?;
    let body = body.map(Value::to_string).unwrap_or_default();
    let (raw_q, enc_q) = (query(params, false)?, query(params, true)?);
    let mut last = None;
    for base in &urls {
        let url = if enc_q.is_empty() {
            format!("{base}{path}")
        } else {
            format!("{base}{path}?{enc_q}")
        };
        let full_path = reqwest::Url::parse(&url)
            .map(|u| u.path().to_string())
            .map_err(|e| {
                ctx.tag(GatewayError::not_set(
                    None,
                    "venue.rest_url",
                    format!("venue.rest_url не адрес: {e}"),
                ))
            })?;
        let mut req = ctx
            .client()?
            .request(m.clone(), url)
            .header("Accept", "application/json");
        if !body.is_empty() {
            req = req
                .header("Content-Type", "application/json")
                .body(body.clone());
        }
        if auth {
            for (k, v) in sign(ctx, &m, &full_path, &raw_q, &body)? {
                req = req.header(k, v);
            }
        }
        match crate::gateway::http::exchange_full(req, lim).await {
            Ok(r) => {
                let parsed = if HEAVY.try_with(|h| *h).unwrap_or(false) {
                    let (c, p) = (ctx.clone(), path.to_string());
                    tokio::task::spawn_blocking(move || reply::parse(&c, &p, &r, op))
                        .await
                        .map_err(|e| {
                            let m = format!("разбор ответа {path} прерван: {e}");
                            ctx.tag(GatewayError::new(ErrorCode::Internal, None, m))
                        })?
                } else {
                    reply::parse(ctx, path, &r, op)
                };
                return parsed.map_err(|e| errors::unsure(e, op));
            }
            Err(f) if may_fallback(&f, op) => last = Some(errors::from_fail(&f, &lim)),
            Err(f) => return Err(errors::fail_for(&f, &lim, op)),
        }
    }
    Err(last.unwrap_or_else(|| {
        ctx.tag(GatewayError::new(
            ErrorCode::NotConfigured,
            None,
            "Не задан адрес REST Gate",
        ))
    }))
}

/// Можно ли повторить запрос на резервном адресе: только если запрос точно не дошёл, а срок без ответа — только для
/// справочников (Op::Other). Ошибок нет.
pub fn may_fallback(f: &crate::gateway::http::Fail, op: Op) -> bool {
    f.not_sent() || (op == Op::Other && f.is_timeout())
}

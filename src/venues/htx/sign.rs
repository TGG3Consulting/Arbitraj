//! Файл: venues/htx/sign.rs
//! Что делает: подпись запросов HTX (Signature V2): строка параметров (кодирование и сортировка), строка для подписи
//! «METHOD\nhost\npath\nпараметры», HMAC-SHA256 → Base64; подписанная строка запроса REST (с параметром Signature) и
//! запрос входа WebSocket v2 (поток аккаунта, signatureVersion 2.1). Секреты не попадают ни в журнал, ни в ошибки.
//! Кто вызывает: rest.rs (каждый приватный запрос), user_stream_conn.rs (вход), тесты.
//! От чего зависит: hmac, sha2, base64, chrono, ctx.rs (Keys).
//! Настройки: нет.
//!
//! Официально («Quick Start → Authentication», https://huobiapi.github.io/docs/spot/v1/en/): метод (GET/POST; у
//! WebSocket — GET) + «\n» + host строчными + «\n» + путь + «\n» + параметры, URL-кодированные (UTF-8, шестнадцатеричные
//! заглавными: «:» → %3A, пробел → %20) и отсортированные по ASCII, через «&». В подпись входят AccessKeyId,
//! SignatureMethod=HmacSHA256, SignatureVersion=2, Timestamp (UTC, «YYYY-MM-DDThh:mm:ss», действует 5 минут). У GET
//! подписываются все параметры, у POST — только эти (тело JSON не подписывается). Подпись — Base64 от сырых байт HMAC;
//! в REST — URL-кодированным параметром Signature, в WebSocket — как есть. Тот же алгоритм — официальный SDK
//! github.com/HuobiRDCenter/huobi_Golang (internal/requestbuilder: signer.go, privateurlbuilder.go,
//! websocketv2requestbuilder.go); векторы его тестов — tests/htx_real_data.rs. Кодирование SDK — url.Values.Encode Go
//! (пробел → «+»): расходится с документацией только на пробеле, в параметрах бота пробелов нет.

use base64::Engine as _;
use chrono::{DateTime, Utc};
use hmac::{Hmac, KeyInit, Mac};
use serde_json::{Value, json};
use sha2::Sha256;

use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::Keys;

/// Метод подписи — официальное значение SignatureMethod / signatureMethod.
pub const METHOD: &str = "HmacSHA256";
/// Версия подписи REST — «SignatureVersion=2».
pub const VERSION_REST: &str = "2";
/// Версия подписи входа WebSocket v2 — «signatureVersion 2.1».
pub const VERSION_WS: &str = "2.1";

/// Что делает: URL-кодирование значения по документации: буквы, цифры и «-_.~» как есть, остальное — %XX заглавными.
/// Принимает: текст. Возвращает: кодированный текст. Ошибок нет.
pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Что делает: строка параметров — пары, отсортированные по имени (ASCII), «имя=значение» кодированными, через «&».
/// Принимает: пары. Возвращает: строку. Ошибок нет.
pub fn encode(params: &[(&str, String)]) -> String {
    let mut sorted: Vec<&(&str, String)> = params.iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(b.0).then_with(|| a.1.cmp(&b.1)));
    sorted
        .iter()
        .map(|(k, v)| format!("{}={}", escape(k), escape(v)))
        .collect::<Vec<_>>()
        .join("&")
}

/// Время подписи в формате документации «YYYY-MM-DDThh:mm:ss» (UTC). Ошибок нет.
pub fn timestamp(at: DateTime<Utc>) -> String {
    at.format("%Y-%m-%dT%H:%M:%S").to_string()
}

/// Что делает: Base64(HMAC-SHA256(METHOD\nhost\npath\nparams)).
/// Принимает: метод, host (приводится к строчным), путь, строку параметров (уже кодированную), секрет.
/// Возвращает: подпись Base64. Ошибки: InvalidCredentials — пустой секрет.
pub fn signature(
    method: &str,
    host: &str,
    path: &str,
    params: &str,
    secret: &str,
) -> GatewayResult<String> {
    let plain = format!(
        "{}\n{}\n{path}\n{params}",
        method.to_ascii_uppercase(),
        host.to_ascii_lowercase()
    );
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).map_err(|_| {
        GatewayError::new(
            ErrorCode::InvalidCredentials,
            None,
            "HTX: пустой секретный ключ",
        )
    })?;
    mac.update(plain.as_bytes());
    Ok(base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes()))
}

/// Что делает: подписанная строка запроса REST: параметры (у GET — все, у POST — передайте пусто) + AccessKeyId,
/// SignatureMethod, SignatureVersion, Timestamp, затем «&Signature=<кодированная подпись>» (как SDK privateurlbuilder.go).
/// Принимает: ключи, метод, host, путь, параметры, время. Возвращает: строку запроса без «?».
/// Ошибки: InvalidCredentials — пустой ключ или секрет.
pub fn signed_query(
    keys: &Keys,
    method: &str,
    host: &str,
    path: &str,
    params: &[(&str, String)],
    at: DateTime<Utc>,
) -> GatewayResult<String> {
    if keys.key.is_empty() {
        return Err(GatewayError::new(
            ErrorCode::InvalidCredentials,
            None,
            "HTX: пустой Access Key (секрет htx.api_key)",
        ));
    }
    let mut all: Vec<(&str, String)> = params.to_vec();
    all.push(("AccessKeyId", keys.key.to_string()));
    all.push(("SignatureMethod", METHOD.to_string()));
    all.push(("SignatureVersion", VERSION_REST.to_string()));
    all.push(("Timestamp", timestamp(at)));
    let q = encode(&all);
    let s = signature(method, host, path, &q, &keys.secret)?;
    Ok(format!("{q}&Signature={}", escape(&s)))
}

/// Что делает: запрос входа WebSocket v2 {"action":"req","ch":"auth","params":{…}} — подпись GET host path с
/// accessKey, signatureMethod, signatureVersion=2.1, timestamp (как SDK websocketv2requestbuilder.go).
/// Принимает: ключи, host, путь (например /ws/v2), время. Возвращает: JSON. Ошибки: InvalidCredentials.
pub fn ws_auth(keys: &Keys, host: &str, path: &str, at: DateTime<Utc>) -> GatewayResult<Value> {
    let ts = timestamp(at);
    let params = vec![
        ("accessKey", keys.key.to_string()),
        ("signatureMethod", METHOD.to_string()),
        ("signatureVersion", VERSION_WS.to_string()),
        ("timestamp", ts.clone()),
    ];
    let s = signature("GET", host, path, &encode(&params), &keys.secret)?;
    Ok(json!({
        "action": "req",
        "ch": "auth",
        "params": {
            "authType": "api",
            "accessKey": keys.key.to_string(),
            "signatureMethod": METHOD,
            "signatureVersion": VERSION_WS,
            "timestamp": ts,
            "signature": s,
        }
    }))
}

/// Host (строчными) и путь из адреса «https://host/путь» или «wss://host/путь». Ошибка — текст причины.
pub fn host_path(url: &str) -> Result<(String, String), String> {
    let rest = url
        .split_once("://")
        .map(|(_, r)| r)
        .ok_or_else(|| format!("адрес {url} без схемы"))?;
    let (host, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    if host.is_empty() {
        return Err(format!("адрес {url} без host"));
    }
    Ok((host.to_ascii_lowercase(), path.to_string()))
}

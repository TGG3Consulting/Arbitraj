//! Файл: venues/okx/sign.rs
//! Что делает: подпись запросов OKX — строка для подписи, HMAC SHA256 → Base64, время в формате OKX и аргументы входа
//! (login) приватного WebSocket. Секреты не попадают ни в журнал, ни в текст ошибок.
//! Кто вызывает: rest.rs (каждый приватный запрос), user_stream_conn.rs (login), тесты.
//! От чего зависит: hmac, sha2, base64, chrono, ctx.rs (Keys).
//! Настройки: нет.
//!
//! Официально (https://www.okx.com/docs-v5/en/, «Overview → REST Authentication → Signature»):
//! - «The OK-ACCESS-SIGN header is generated as follows: Create a pre-hash string of timestamp + method + requestPath +
//!   body … Sign the pre-hash string with the SecretKey using the HMAC SHA256. Encode the signature in the Base64 format»;
//! - «GET request parameters are counted as requestpath, not body» (пример — `/api/v5/account/balance?ccy=BTC`);
//! - OK-ACCESS-TIMESTAMP — «ISO 8601 UTC format with millisecond precision, e.g. 2020-12-08T09:08:57.715Z»; расхождение
//!   больше 30 с — код 50102 «Timestamp request expired».
//!
//! «Overview → WebSocket → Login»: timestamp — «Unix Epoch time, the unit is seconds»; sign — та же схема, «method is
//! always 'GET'», «requestPath is always '/users/self/verify'».
//! Тот же алгоритм — официальный SDK github.com/okxapi/python-okx v0.4.4: okx/utils.py (signature, get_timestamp),
//! okx/websocket/WsUtils.py (initLoginParams). Проверочные значения — tests/okx_real_data.rs.

use base64::Engine as _;
use chrono::{DateTime, Utc};
use hmac::{Hmac, KeyInit, Mac};
use serde_json::{Value, json};
use sha2::Sha256;

use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::Keys;

/// Путь подписи входа приватного WebSocket — постоянная протокола («requestPath is always '/users/self/verify'»).
pub const WS_LOGIN_PATH: &str = "/users/self/verify";

/// Что делает: Base64(HMAC-SHA256(plain, key)).
/// Принимает: текст и ключ. Возвращает: подпись Base64 (стандартный алфавит с «=»).
/// Ошибки: InvalidCredentials — пустой ключ (HMAC его не примет).
pub fn hmac_b64(plain: &[u8], key: &[u8]) -> GatewayResult<String> {
    if key.is_empty() {
        return Err(GatewayError::new(
            ErrorCode::InvalidCredentials,
            None,
            "OKX: пустой секретный ключ (секрет okx.api_secret)",
        ));
    }
    let mut mac = Hmac::<Sha256>::new_from_slice(key).map_err(|_| {
        GatewayError::new(
            ErrorCode::InvalidCredentials,
            None,
            "OKX: секретный ключ не принят HMAC",
        )
    })?;
    mac.update(plain);
    Ok(base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes()))
}

/// Строка для подписи: timestamp + METHOD + путь(со строкой запроса) + тело. Ошибок нет.
pub fn prehash(ts: &str, method: &str, path_query: &str, body: &str) -> String {
    format!("{ts}{}{path_query}{body}", method.to_ascii_uppercase())
}

/// Время запроса REST: ISO 8601 UTC с миллисекундами и «Z» (как get_timestamp SDK). Ошибок нет.
pub fn rest_timestamp(at: DateTime<Utc>) -> String {
    at.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
}

/// Заголовки приватного запроса REST.
pub struct Headers {
    pub key: String,
    pub sign: String,
    pub timestamp: String,
    pub passphrase: String,
}

/// Что делает: заголовки OK-ACCESS-* для запроса.
/// Принимает: ключи, время ISO (rest_timestamp), метод, путь со строкой запроса, тело.
/// Возвращает: Headers. Ошибки: InvalidCredentials — пустой ключ, секрет или passphrase.
pub fn headers(
    keys: &Keys,
    ts: &str,
    method: &str,
    path_query: &str,
    body: &str,
) -> GatewayResult<Headers> {
    check(keys)?;
    let plain = prehash(ts, method, path_query, body);
    Ok(Headers {
        key: keys.key.to_string(),
        sign: hmac_b64(plain.as_bytes(), keys.secret.as_bytes())?,
        timestamp: ts.to_string(),
        passphrase: keys.passphrase.to_string(),
    })
}

/// Что делает: запрос входа приватного WebSocket {"op":"login","args":[{apiKey, passphrase, timestamp, sign}]}.
/// Принимает: ключи, время unix в секундах. Возвращает: JSON запроса (содержит ключ и passphrase — в журнал не писать).
/// Ошибки: InvalidCredentials — пустой ключ, секрет или passphrase.
pub fn ws_login(keys: &Keys, ts_s: i64) -> GatewayResult<Value> {
    check(keys)?;
    let ts = ts_s.to_string();
    let plain = prehash(&ts, "GET", WS_LOGIN_PATH, "");
    let sign = hmac_b64(plain.as_bytes(), keys.secret.as_bytes())?;
    Ok(json!({"op": "login", "args": [{
        "apiKey": keys.key.as_str(),
        "passphrase": keys.passphrase.as_str(),
        "timestamp": ts,
        "sign": sign,
    }]}))
}

/// Ключ и passphrase не пустые. Ошибка InvalidCredentials — с именем пустого секрета.
fn check(keys: &Keys) -> GatewayResult<()> {
    let empty = if keys.key.is_empty() {
        Some("okx.api_key")
    } else if keys.passphrase.is_empty() {
        Some("okx.api_passphrase")
    } else {
        None
    };
    match empty {
        Some(name) => Err(GatewayError::new(
            ErrorCode::InvalidCredentials,
            None,
            format!("OKX: пустой секрет {name}"),
        )),
        None => Ok(()),
    }
}

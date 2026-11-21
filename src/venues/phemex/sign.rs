//! Файл: venues/phemex/sign.rs
//! Что делает: подпись приватных запросов Phemex — строка для подписи REST (путь + строка запроса + expiry + тело),
//! HMAC SHA256 → hex, заголовки x-phemex-*; подпись входа WebSocket (user.auth). Секреты не попадают ни в журнал, ни в
//! текст ошибок.
//! Кто вызывает: rest.rs (каждый приватный запрос), user_stream_conn.rs (вход), тесты.
//! От чего зависит: hmac, sha2, hex, ctx.rs (Keys).
//! Настройки: нет (срок expiry передаёт вызывающий).
//!
//! Официально (General API information, https://phemex-docs.github.io/#rest-request-header и #endpoint-security-type):
//! - x-phemex-access-token — ключ; x-phemex-request-expiry — «Unix EPoch SECONDS to expire the request, normally it
//!   should be (Now() + 1 minute)»; x-phemex-request-signature — «HMacSha256(URL Path + QueryString + Expiry + body)»,
//!   ключ — API Secret, «The signature is case sensitive». Примеры строк для подписи («Signed string») — без «?» между
//!   путём и строкой запроса (пример 2: «/orders/activeListordStatus=New&…&symbol=BTCUSD1575735951»).
//! - Вид результата (hex) в тексте не назван; официальный пример клиента github.com/phemex/phemex-python-api
//!   (phemex/client.py, _send_request) берёт hmac.new(secret, message, sha256).hexdigest() — строчный hex; подпись входа в
//!   примере user.auth — 64 hex-знака. Вектор — tests/phemex_real_data.rs.
//! - Spot Websocket API → User authentication (https://phemex-docs.github.io/#user-authentication-2): params ["API",
//!   ключ, подпись, expiry], подпись «HMacSha256(API Key + expiry) with API Secret», expiry — секунды, «Maximum expiry is
//!   request time plus 2 minutes».

use hmac::{Hmac, KeyInit, Mac};
use serde_json::{Value, json};
use sha2::Sha256;

use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::Keys;

/// Что делает: hex(HMAC-SHA256(plain, key)) строчными буквами.
/// Принимает: текст и ключ. Возвращает: подпись. Ошибки: InvalidCredentials — пустой ключ.
pub fn hmac_hex(plain: &[u8], key: &[u8]) -> GatewayResult<String> {
    if key.is_empty() {
        return Err(GatewayError::new(
            ErrorCode::InvalidCredentials,
            None,
            "Phemex: пустой секретный ключ",
        ));
    }
    let mut mac = Hmac::<Sha256>::new_from_slice(key).map_err(|_| {
        GatewayError::new(
            ErrorCode::InvalidCredentials,
            None,
            "Phemex: секретный ключ не принят HMAC",
        )
    })?;
    mac.update(plain);
    Ok(hex::encode(mac.finalize().into_bytes()))
}

/// Строка для подписи REST: путь + строка запроса (без «?») + expiry + тело. Ошибок нет.
pub fn prehash(path: &str, query: &str, expiry: i64, body: &str) -> String {
    format!("{path}{query}{expiry}{body}")
}

/// Заголовки приватного запроса.
pub struct Headers {
    pub token: String,
    pub expiry: String,
    pub signature: String,
}

/// Что делает: заголовки x-phemex-* запроса.
/// Принимает: ключи, путь, строку запроса, expiry (секунды unix), тело. Возвращает: Headers.
/// Ошибки: InvalidCredentials — пустой ключ или секрет.
pub fn headers(
    keys: &Keys,
    path: &str,
    query: &str,
    expiry: i64,
    body: &str,
) -> GatewayResult<Headers> {
    if keys.key.is_empty() {
        return Err(GatewayError::new(
            ErrorCode::InvalidCredentials,
            None,
            "Phemex: пустой API-ключ (секрет phemex.api_key)",
        ));
    }
    let plain = prehash(path, query, expiry, body);
    Ok(Headers {
        token: keys.key.to_string(),
        expiry: expiry.to_string(),
        signature: hmac_hex(plain.as_bytes(), keys.secret.as_bytes())?,
    })
}

/// Что делает: сообщение входа WebSocket user.auth.
/// Принимает: ключи, expiry (секунды unix), id сообщения. Возвращает: JSON сообщения (с ключом — в журнал не писать).
/// Ошибки: InvalidCredentials — пустой ключ или секрет.
pub fn ws_auth(keys: &Keys, expiry: i64, id: u64) -> GatewayResult<Value> {
    let plain = format!("{}{expiry}", keys.key.as_str());
    let signature = hmac_hex(plain.as_bytes(), keys.secret.as_bytes())?;
    Ok(json!({
        "method": "user.auth",
        "params": ["API", keys.key.as_str(), signature, expiry],
        "id": id,
    }))
}

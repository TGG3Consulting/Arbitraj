//! Файл: venues/coinex/sign.rs
//! Что делает: подпись CoinEx API v2 — строка для подписи REST (METHOD + путь со строкой запроса + тело + время) и
//! WebSocket (только время), HMAC-SHA256 секретным ключом → hex строчными (64 знака). Секреты не попадают ни в журнал,
//! ни в текст ошибок.
//! Кто вызывает: rest.rs (каждый подписанный запрос), тесты.
//! От чего зависит: hmac, sha2, hex, ctx.rs (Keys).
//! Настройки: нет.
//!
//! Официально (Authentication, https://docs.coinex.com/api/v2/authorization):
//! - строка HTTP: «HTTP method, uppercase» + «Request path, including the query string if present» + «Request body
//!   (optional; can be omitted for GET/DELETE)» + «Timestamp, taken from X-COINEX-TIMESTAMP»; пример:
//!   "GET" + "/v2/spot/pending-order?market=BTCUSDT&market_type=SPOT&side=buy&page=1&limit=10" + "1700490703564" — путь
//!   с «/v2» (rest.rs шлёт и подписывает путь целиком);
//! - строка WebSocket: только timestamp («prepared_str = "1700490703564"»);
//! - hmac.new(bytes(secret_key, 'latin-1'), msg=bytes(prepared_str, 'latin-1'), digestmod=hashlib.sha256)
//!   .hexdigest().lower() — hex строчными, 64 знака. Пример ключа и готовой подписи на странице нет (ключи —
//!   «XXXXXXXXXX»): тест — строка документации и вектор HMAC-SHA256 RFC 4231 (tests/coinex_real_data.rs).
//! - заголовки: X-COINEX-KEY (access_id), X-COINEX-SIGN, X-COINEX-TIMESTAMP (мс).

use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;

use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::Keys;

/// Что делает: hex строчными от HMAC-SHA256(plain, key).
/// Принимает: текст и ключ. Возвращает: 64 знака [0-9a-f].
/// Ошибки: InvalidCredentials — пустой ключ (HMAC его не примет).
pub fn hmac_hex(plain: &[u8], key: &[u8]) -> GatewayResult<String> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).map_err(|_| {
        GatewayError::new(
            ErrorCode::InvalidCredentials,
            None,
            "CoinEx: пустой секретный ключ (coinex.api_secret)",
        )
    })?;
    mac.update(plain);
    Ok(hex::encode(mac.finalize().into_bytes()))
}

/// Строка для подписи REST: METHOD + путь(со строкой запроса) + тело + время. Ошибок нет.
pub fn prehash(method: &str, path_query: &str, body: &str, ts_ms: i64) -> String {
    format!("{}{path_query}{body}{ts_ms}", method.to_ascii_uppercase())
}

/// Заголовки подписанного запроса.
pub struct Headers {
    pub key: String,
    pub sign: String,
    pub timestamp: String,
}

/// Что делает: заголовки X-COINEX-* для запроса.
/// Принимает: ключи, метод, путь со строкой запроса, тело, время мс. Возвращает: Headers.
/// Ошибки: InvalidCredentials — пустой ключ или секрет.
pub fn headers(
    keys: &Keys,
    method: &str,
    path_query: &str,
    body: &str,
    ts_ms: i64,
) -> GatewayResult<Headers> {
    if keys.key.is_empty() {
        return Err(GatewayError::new(
            ErrorCode::InvalidCredentials,
            None,
            "CoinEx: пустой access_id (секрет coinex.api_key)",
        ));
    }
    let plain = prehash(method, path_query, body, ts_ms);
    Ok(Headers {
        key: keys.key.to_string(),
        sign: hmac_hex(plain.as_bytes(), keys.secret.as_bytes())?,
        timestamp: ts_ms.to_string(),
    })
}

/// Подпись входа WebSocket (server.sign): HMAC от строки времени. Принимает: ключи, время мс.
/// Возвращает: signed_str. Ошибки: InvalidCredentials.
pub fn ws_signed(keys: &Keys, ts_ms: i64) -> GatewayResult<String> {
    hmac_hex(ts_ms.to_string().as_bytes(), keys.secret.as_bytes())
}

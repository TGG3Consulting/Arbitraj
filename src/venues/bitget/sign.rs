//! Файл: venues/bitget/sign.rs
//! Что делает: подпись приватного запроса Bitget (Classic v2) — строка для подписи, HMAC SHA256 → Base64, заголовки
//! ACCESS-*. Секреты не попадают ни в журнал, ни в текст ошибок.
//! Кто вызывает: rest.rs (каждый приватный запрос), тесты.
//! От чего зависит: hmac, sha2, base64, ctx.rs (Keys).
//! Настройки: нет.
//!
//! Официально (Classic → REST API → Signature, https://www.bitget.com/docs/classic/rest-api): строка для подписи —
//! «timestamp + method.toUpperCase() + requestPath + "?" + queryString + body» («?» и строки запроса нет, если она
//! пуста; у GET тела нет), HMAC SHA256 секретным ключом, затем Base64. ACCESS-TIMESTAMP — миллисекунды, «within 30
//! seconds of the API server time». ACCESS-PASSPHRASE — passphrase как есть (без шифрования, в отличие от KuCoin).
//! Тот же алгоритм — официальный SDK github.com/BitgetLimited/v3-bitget-api-sdk (bitget-python-sdk-api/bitget/utils.py:
//! pre_hash, sign); вектор, посчитанный функциями SDK, — tests/bitget_real_data.rs.

use base64::Engine as _;
use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;

use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::Keys;

/// Что делает: Base64(HMAC-SHA256(plain, key)).
/// Принимает: текст и ключ. Возвращает: подпись Base64 (стандартный алфавит с «=»).
/// Ошибки: InvalidCredentials — пустой ключ (HMAC его не примет).
pub fn hmac_b64(plain: &[u8], key: &[u8]) -> GatewayResult<String> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).map_err(|_| {
        GatewayError::new(
            ErrorCode::InvalidCredentials,
            None,
            "Bitget: пустой секретный ключ",
        )
    })?;
    mac.update(plain);
    Ok(base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes()))
}

/// Строка для подписи: timestamp + METHOD + путь(со строкой запроса, если она есть) + тело. Ошибок нет.
pub fn prehash(ts_ms: i64, method: &str, path_query: &str, body: &str) -> String {
    format!("{ts_ms}{}{path_query}{body}", method.to_ascii_uppercase())
}

/// Заголовки приватного запроса.
pub struct Headers {
    pub key: String,
    pub sign: String,
    pub timestamp: String,
    pub passphrase: String,
}

/// Что делает: заголовки ACCESS-* для запроса.
/// Принимает: ключи, время мс, метод, путь со строкой запроса, тело. Возвращает: Headers.
/// Ошибки: InvalidCredentials — пустой секрет или пустая passphrase.
pub fn headers(
    keys: &Keys,
    ts_ms: i64,
    method: &str,
    path_query: &str,
    body: &str,
) -> GatewayResult<Headers> {
    if keys.passphrase.is_empty() {
        return Err(GatewayError::new(
            ErrorCode::InvalidCredentials,
            None,
            "Bitget: пустая passphrase ключа (секрет bitget.api_passphrase)",
        ));
    }
    let plain = prehash(ts_ms, method, path_query, body);
    Ok(Headers {
        key: keys.key.to_string(),
        sign: hmac_b64(plain.as_bytes(), keys.secret.as_bytes())?,
        timestamp: ts_ms.to_string(),
        passphrase: keys.passphrase.to_string(),
    })
}

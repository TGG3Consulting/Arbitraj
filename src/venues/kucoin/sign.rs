//! Файл: venues/kucoin/sign.rs
//! Что делает: подпись приватного запроса KuCoin — строка для подписи, HMAC SHA256 → Base64, зашифрованная passphrase
//! и заголовки KC-API-*. Секреты не попадают ни в журнал, ни в текст ошибок.
//! Кто вызывает: rest.rs (каждый приватный запрос), user_stream.rs (через rest.rs — bullet-private), тесты.
//! От чего зависит: hmac, sha2, base64, ctx.rs (Keys).
//! Настройки: venue.api_key_version [kucoin] — версия ключа (KC-API-KEY-VERSION), её показывает страница API Management.
//!
//! Официально (https://www.kucoin.com/docs-new/authentication):
//! - «KC-API-SIGN»: Base64 от HMAC-SHA256 строки «timestamp + method + endpoint + body» секретом ключа;
//! - метод — заглавными; endpoint — путь со строкой запроса (у GET/DELETE параметры в адресе, тело — «»), у POST —
//!   параметры в теле JSON без лишних пробелов; подписывается адрес ДО URL-кодирования;
//! - «KC-API-PASSPHRASE»: passphrase, зашифрованная HMAC-SHA256 секретом ключа и закодированная Base64;
//! - «KC-API-TIMESTAMP» — миллисекунды, тот же, что в подписи; «KC-API-KEY-VERSION» — версия ключа (в примерах «3»).
//!
//! Тот же алгоритм — официальный SDK github.com/Kucoin/kucoin-universal-sdk (sdk/golang/internal/infra/default_signer.go),
//! его тестовый вектор — tests/kucoin_real_data.rs.

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
            "KuCoin: пустой секретный ключ",
        )
    })?;
    mac.update(plain);
    Ok(base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes()))
}

/// Строка для подписи: timestamp + METHOD + путь(со строкой запроса) + тело. Ошибок нет.
pub fn prehash(ts_ms: i64, method: &str, path_query: &str, body: &str) -> String {
    format!("{ts_ms}{}{path_query}{body}", method.to_ascii_uppercase())
}

/// Заголовки приватного запроса.
pub struct Headers {
    pub key: String,
    pub sign: String,
    pub timestamp: String,
    pub passphrase: String,
    pub version: String,
}

/// Что делает: заголовки KC-API-* для запроса.
/// Принимает: ключи, версию ключа (venue.api_key_version), время мс, метод, путь со строкой запроса, тело.
/// Возвращает: Headers. Ошибки: InvalidCredentials — пустой секрет или пустая passphrase.
pub fn headers(
    keys: &Keys,
    version: &str,
    ts_ms: i64,
    method: &str,
    path_query: &str,
    body: &str,
) -> GatewayResult<Headers> {
    if keys.passphrase.is_empty() {
        return Err(GatewayError::new(
            ErrorCode::InvalidCredentials,
            None,
            "KuCoin: пустая passphrase ключа (секрет kucoin.api_passphrase)",
        ));
    }
    let secret = keys.secret.as_bytes();
    let plain = prehash(ts_ms, method, path_query, body);
    Ok(Headers {
        key: keys.key.to_string(),
        sign: hmac_b64(plain.as_bytes(), secret)?,
        timestamp: ts_ms.to_string(),
        passphrase: hmac_b64(keys.passphrase.as_bytes(), secret)?,
        version: version.to_string(),
    })
}

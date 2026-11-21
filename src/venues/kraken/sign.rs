//! Файл: venues/kraken/sign.rs
//! Что делает: подпись запросов Kraken — API-Sign = Base64(HMAC-SHA512(путь + SHA256(nonce + данные), Base64-декод
//! секрета)) — и следующий nonce (миллисекунды, строго больше прежнего). Секреты не попадают ни в журнал, ни в ошибки.
//! Кто вызывает: rest.rs (каждый подписанный запрос), тесты.
//! От чего зависит: hmac, sha2, base64, ctx.rs (Keys).
//! Настройки: нет.
//!
//! Официально:
//! - спот REST (https://docs.kraken.com/exchange/guides/rest/authentication.md): заголовки API-Key, API-Sign; nonce — в
//!   теле; «HMAC-SHA512 of (URI path + SHA256(nonce + POST data)) and base64 decoded secret API key», путь — /0/private/…;
//!   проверочный пример гида (AddOrder, nonce 1616492376594) — tests/kraken_real_data.rs;
//! - Funding (Beta) (https://docs.kraken.com/exchange/guides/rest/funding.md, помощник гида): заголовки api-key,
//!   api-sign, api-nonce; «the signed path includes the query string» (`signed_path = path?query`), тело — компактный
//!   JSON (`separators=(",", ":")`) или пусто; `sha256(nonce + body)`, `hmac(b64decode(secret), signed_path + digest,
//!   sha512)`; nonce помощника — `max(int(time*1000), last + 1)`.
//!
//! Формула у обоих одна — `api_sign(путь, nonce, данные)`; разница только в том, что считается «путём» и «данными».

use base64::Engine as _;
use hmac::{Hmac, KeyInit, Mac};
use sha2::{Digest, Sha256, Sha512};

use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::Keys;

/// Что делает: следующий nonce — текущее время в мс, но строго больше прежнего (как помощник гида Funding).
/// Принимает: последний выданный nonce, сейчас в мс. Возвращает: новый nonce. Ошибок нет.
pub fn next_nonce(last: u64, now_ms: u64) -> u64 {
    now_ms.max(last + 1)
}

/// Что делает: API-Sign. Принимает: путь (у Funding — со строкой запроса), nonce строкой, данные (тело формы у спота,
/// JSON у Funding, пусто), секрет Base64. Возвращает: подпись Base64.
/// Ошибки: InvalidCredentials — секрет не Base64 или пустой.
pub fn api_sign(path: &str, nonce: &str, data: &[u8], secret_b64: &str) -> GatewayResult<String> {
    let bad = |m: &str| {
        GatewayError::new(
            ErrorCode::InvalidCredentials,
            None,
            format!("Kraken: {m} (секрет kraken.api_secret)"),
        )
    };
    let key = base64::engine::general_purpose::STANDARD
        .decode(secret_b64.trim())
        .map_err(|_| bad("приватный ключ не в Base64"))?;
    if key.is_empty() {
        return Err(bad("пустой приватный ключ"));
    }
    let mut sha = Sha256::new();
    sha.update(nonce.as_bytes());
    sha.update(data);
    let digest = sha.finalize();
    let mut mac =
        Hmac::<Sha512>::new_from_slice(&key).map_err(|_| bad("приватный ключ не принят HMAC"))?;
    mac.update(path.as_bytes());
    mac.update(&digest);
    Ok(base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes()))
}

/// Ключ не пустой. Ошибка InvalidCredentials. Ошибок больше нет.
pub fn check(keys: &Keys) -> GatewayResult<()> {
    if keys.key.trim().is_empty() {
        return Err(GatewayError::new(
            ErrorCode::InvalidCredentials,
            None,
            "Kraken: пустой секрет kraken.api_key",
        ));
    }
    Ok(())
}

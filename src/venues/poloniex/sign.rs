//! Файл: venues/poloniex/sign.rs
//! Что делает: подпись приватного запроса Poloniex (REST и вход в приватный WebSocket) — строка для подписи,
//! HmacSHA256 → Base64, кодирование параметров как в официальном SDK. Секреты не попадают ни в журнал, ни в текст
//! ошибок.
//! Кто вызывает: rest.rs (каждый приватный запрос), user_stream.rs (сообщение auth), тесты.
//! От чего зависит: hmac, sha2, base64, ctx.rs (Keys).
//! Настройки: нет.
//!
//! Официально (Spot → API → Authentication, https://api-docs.poloniex.com/spot/api/): строка — «METHOD\n» + «путь\n» +
//! параметры. GET и запрос без тела: все параметры вместе с signTimestamp, по возрастанию имени (ASCII), «имя=значение»
//! через «&», значения URL-кодированы («a space will be encoded as %20»). POST/DELETE с телом: «requestBody=<JSON>&
//! signTimestamp=<мс>». HmacSHA256 секретом, затем Base64. Тот же алгоритм — официальный SDK
//! github.com/poloniex/polo-sdk-python (polosdk/spot/rest/request.py: encode_uri_component, _get_sig_header); вектор,
//! посчитанный его функциями, — tests/poloniex_real_data.rs.
//! Вход в приватный WebSocket (Spot → WebSocket → Authentication, https://api-docs.poloniex.com/spot/websocket/
//! authentication; SDK polosdk/spot/ws/client_authenticated.py: _get_signature): строка «GET\n/ws\nsignTimestamp=<мс>».
//! signatureMethod / signatureVersion не шлются (необязательны; в документации версия то «1», то «2» — урок 2), как SDK.

use base64::Engine as _;
use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;

use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

/// Знаки, которые SDK не кодирует: буквы, цифры, «_.-~» (urllib.parse.quote) и safe='~()*!\''.
fn unreserved(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b"_.-~()*!'".contains(&b)
}

/// Что делает: URL-кодирование значения как encode_uri_component SDK (UTF-8, «%XX» заглавными). Ошибок нет.
pub fn encode(v: &str) -> String {
    let mut out = String::with_capacity(v.len());
    for b in v.bytes() {
        if unreserved(b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Что делает: параметры по возрастанию имени, «имя=кодированное значение» через «&». Ошибок нет.
pub fn sorted_query(params: &[(&str, String)]) -> String {
    let mut sorted: Vec<&(&str, String)> = params.iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(b.0));
    sorted
        .iter()
        .map(|(k, v)| format!("{k}={}", encode(v)))
        .collect::<Vec<_>>()
        .join("&")
}

/// Что делает: строка для подписи. Принимает: метод, путь (без строки запроса), параметры строки запроса, тело (пусто —
/// нет тела), метку времени мс. Возвращает: текст «METHOD\nпуть\nпараметры». Ошибок нет.
pub fn payload(method: &str, path: &str, query: &[(&str, String)], body: &str, ts: i64) -> String {
    let params = if body.is_empty() {
        let mut all: Vec<(&str, String)> = query.to_vec();
        all.push(("signTimestamp", ts.to_string()));
        sorted_query(&all)
    } else {
        format!("requestBody={body}&signTimestamp={ts}")
    };
    format!("{}\n{path}\n{params}", method.to_ascii_uppercase())
}

/// Что делает: строка входа в приватный WebSocket «GET\n/ws\nsignTimestamp=<мс>». Ошибок нет.
pub fn ws_payload(ts: i64) -> String {
    format!("GET\n/ws\nsignTimestamp={ts}")
}

/// Что делает: Base64(HMAC-SHA256(plain, key)).
/// Принимает: текст и ключ. Возвращает: подпись Base64 (стандартный алфавит с «=»).
/// Ошибки: InvalidCredentials — пустой ключ.
pub fn hmac_b64(plain: &[u8], key: &[u8]) -> GatewayResult<String> {
    if key.is_empty() {
        return Err(empty());
    }
    let mut mac = Hmac::<Sha256>::new_from_slice(key).map_err(|_| empty())?;
    mac.update(plain);
    Ok(base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes()))
}

/// Ошибка «пустой секрет».
fn empty() -> GatewayError {
    GatewayError::new(
        ErrorCode::InvalidCredentials,
        None,
        "Poloniex: пустой секретный ключ",
    )
}

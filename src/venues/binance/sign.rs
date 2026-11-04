//! Файл: venues/binance/sign.rs
//! Что делает: подпись запросов Binance HMAC SHA256 (hex) — строка запроса REST (в порядке отправки) и строка
//! параметров WebSocket API (по алфавиту имени). Чистые функции — проверяются официальными примерами документации.
//! Кто вызывает: rest.rs (REST и SAPI), user_stream_conn.rs (userDataStream.subscribe.signature), тесты.
//! От чего зависит: hmac, sha2, hex.
//! Настройки: нет.
//!
//! Официально:
//! - REST https://developers.binance.com/legacy-docs/binance-spot-api-docs/rest-api/request-security — «payload =
//!   query string + request body» без разделителя, не-ASCII — percent-encode до подписи, HMAC-SHA-256 секретом, hex.
//!   Пример: секрет NhqPtmdSJYdKjVHjA7PZj4Mge3R5YNiP1e3UZjInClVN65XAbvqqM6A7H5fATj0j, строка
//!   symbol=LTCBTC&side=BUY&type=LIMIT&timeInForce=GTC&quantity=1&price=0.1&recvWindow=5000&timestamp=1499827319559 →
//!   c8db56825ae71d6d79447849e617115f4a920fa2acdcab2b053c4b2838bd6b71 (тест tests/binance_real_data.rs).
//!   Подпись SAPI (Wallet) — та же (Wallet «General Info»).
//! - WebSocket API https://developers.binance.com/legacy-docs/binance-spot-api-docs/websocket-api/request-security —
//!   все params, кроме signature, «sort them in alphabetical order by parameter name», «parameter=value» через «&»,
//!   значения UTF-8 без percent-encoding; пример с apiKey vmPUZE6m… → aa1b5712c094bc4e57c05a1a5c1fd8d88dcd628338ea863fec7b88e59fe2db24.

use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;

/// Что делает: HMAC SHA256 строки секретом, результат — hex строчными.
/// Принимает: секрет, строку. Возвращает: подпись. Ошибка — пустой секрет (текст причины).
pub fn hmac_hex(secret: &str, payload: &str) -> Result<String, String> {
    if secret.is_empty() {
        return Err("пустой секретный ключ Binance".into());
    }
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes())
        .map_err(|e| format!("секретный ключ Binance не принят HMAC: {e}"))?;
    mac.update(payload.as_bytes());
    Ok(hex::encode(mac.finalize().into_bytes()))
}

/// Что делает: строка параметров REST «k=v&k=v» в порядке передачи (он же порядок подписи). Буквы, цифры и «-_.~»
/// как есть, остальное — %XX в верхнем регистре (не-ASCII документация требует кодировать до подписи; прочее
/// кодируется одинаково в отправке и подписи).
/// Принимает: пары имя — значение. Возвращает: строку. Ошибка — перевод строки в значении (текст причины).
pub fn query(params: &[(&str, String)]) -> Result<String, String> {
    let mut out = Vec::with_capacity(params.len());
    for (k, v) in params {
        if v.contains(['\r', '\n']) {
            return Err(format!("недопустимый символ в параметре {k}"));
        }
        out.push(format!("{k}={}", encode(v)));
    }
    Ok(out.join("&"))
}

/// Что делает: строка для подписи запроса WebSocket API — параметры по алфавиту имени, «k=v» через «&», без
/// кодирования. Принимает: пары имя — значение (без signature). Возвращает: строку. Ошибок нет.
pub fn ws_payload(params: &[(&str, String)]) -> String {
    let mut sorted: Vec<&(&str, String)> = params.iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(b.0));
    sorted
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("&")
}

/// Кодирование значения строки запроса. Ошибок нет.
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

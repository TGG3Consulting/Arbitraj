//! Файл: venues/xt/sign.rs
//! Что делает: подпись запросов XT.com — строка для подписи original = X + Y и HMAC SHA256 (hex) секретом ключа.
//! Спот: X — заголовки validate-algorithms, validate-appkey, validate-recvwindow, validate-timestamp по алфавиту
//! «k=v» через «&»; Y = «#METHOD#path#query#body» (пустые части пропускаются). Фьючерсы (панель «Балансы»): X —
//! только validate-appkey и validate-timestamp, Y = «#path#query#body» без метода. Чистые функции — тесты сверяют
//! сборку строки с примером документации.
//! Кто вызывает: rest.rs (спот и фьючерсы), тесты (tests/xt_real_data.rs).
//! От чего зависит: hmac, sha2, hex.
//! Настройки: нет.
//!
//! Официально:
//! - спот https://doc.xt.com/#documentationsignSteps (`_documentation/signSteps.md`): «query: Sort all key=value
//!   according to the lexicographical order of the key»; «Json: Directly by JSON string without conversion or
//!   sorting»; «Y=#method#path#query#body»; «After the keys are in natural ascending alphabetical order, use & to join
//!   them together as X»; «signature = hmacSha256Hex(secretkey, original)», original = XY. Пример строки:
//!   validate-algorithms=HmacSHA256&validate-appkey=2063495b-…&validate-recvwindow=60000&validate-timestamp=1666026215729
//!   #POST#/v4/order#{"symbol":"XT_USDT",…} (тест). Подписи в примерах документации воспроизвести нельзя: секрет второго
//!   примера не опубликован, тело первого дано не в JSON (вопрос Тиграну в отчёте решения 167).
//! - фьючерсы https://doc.xt.com/#futures_documentationsignSteps (`_futures_documentation/signSteps.md`):
//!   X = «validate-appkey=…&validate-timestamp=…», «Y=#path#query#body».
//! - Расхождение: официальный SDK pyxt (pyxt/spot.py) шлёт заголовки «xt-validate-*» и подпись заглавными hex —
//!   адаптер следует документации (validate-*, строчные hex); проверить на живом ответе (AUTH_103 — подпись).

use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;

/// Алгоритм подписи — рекомендованный документацией («HmacSHA256 is recommended»).
pub const ALGORITHM: &str = "HmacSHA256";

/// Что делает: X спота — заголовки по алфавиту имени. Принимает: ключ, recvWindow (мс), время (мс). Ошибок нет.
pub fn spot_headers(appkey: &str, recv_window_ms: u64, timestamp_ms: i64) -> String {
    format!(
        "validate-algorithms={ALGORITHM}&validate-appkey={appkey}&validate-recvwindow={recv_window_ms}&validate-timestamp={timestamp_ms}"
    )
}

/// Что делает: X фьючерсов — validate-appkey и validate-timestamp. Ошибок нет.
pub fn futures_headers(appkey: &str, timestamp_ms: i64) -> String {
    format!("validate-appkey={appkey}&validate-timestamp={timestamp_ms}")
}

/// Что делает: Y — «#» перед каждой непустой частью (метод, путь, строка запроса, тело) по порядку.
/// Принимает: части (метод пустой — у фьючерсов). Возвращает: строку. Ошибок нет.
pub fn data(method: &str, path: &str, query: &str, body: &str) -> String {
    let mut y = String::with_capacity(path.len() + query.len() + body.len() + 8);
    for part in [method, path, query, body] {
        if !part.is_empty() {
            y.push('#');
            y.push_str(part);
        }
    }
    y
}

/// Что делает: строка запроса — пары «k=v» по алфавиту имени через «&» (она же уходит в адрес запроса). Значения без
/// кодирования: документация не говорит, кодируются ли они до подписи, поэтому допускаются только буквы, цифры и
/// «_ . , -» (иное — отказ до отправки).
/// Принимает: пары имя — значение. Возвращает: строку. Ошибка — текст причины (недопустимый символ).
pub fn query(params: &[(&str, String)]) -> Result<String, String> {
    let mut sorted: Vec<&(&str, String)> = params.iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(b.0));
    let mut out = Vec::with_capacity(sorted.len());
    for (k, v) in sorted {
        let ok = v
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b',' | b'-'));
        if !ok {
            return Err(format!(
                "параметр {k} = «{v}»: недопустимый символ для подписи XT"
            ));
        }
        out.push(format!("{k}={v}"));
    }
    Ok(out.join("&"))
}

/// Что делает: HMAC SHA256 строки секретом, результат — hex строчными (hmacSha256Hex документации).
/// Принимает: секрет, строку. Возвращает: подпись. Ошибка — пустой секрет (текст причины).
pub fn hmac_hex(secret: &str, original: &str) -> Result<String, String> {
    if secret.is_empty() {
        return Err("пустой секретный ключ XT".into());
    }
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes())
        .map_err(|e| format!("секретный ключ XT не принят HMAC: {e}"))?;
    mac.update(original.as_bytes());
    Ok(hex::encode(mac.finalize().into_bytes()))
}

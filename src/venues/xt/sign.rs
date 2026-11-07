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
        if !value_ok(v) {
            return Err(format!(
                "параметр {k} = «{v}»: недопустимый символ для подписи XT"
            ));
        }
        out.push(format!("{k}={v}"));
    }
    Ok(out.join("&"))
}

/// Решение 207: как подписывать строку запроса со значением, которое нельзя поставить в адрес как есть (пробел в имени
/// сети «BNB Smart Chain» — живой отказ 07.11.2026 при запросе адреса депозита). Документация XT («Sort all key=value
/// according to the lexicographical order of the key») не говорит, кодируется ли значение до подписи; поэтому в адрес
/// значение уходит закодированным (RFC 3986, пробел — %20), а подписывается — закодированная строка (Encoded) или
/// исходная (Raw). Какую принимает XT, выясняется по живому ответу (rest.rs: отказ подписи AUTH_103 — второй вариант).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignForm {
    Encoded,
    Raw,
}

/// Знаки, которые кодируются в значении: всё, кроме букв, цифр и «- . _ ~» (RFC 3986, unreserved).
const ENC: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

/// Что делает: строки запроса для адреса и для подписи. Все значения «простые» (value_ok) — обе строки одинаковы,
/// как раньше. Иначе в адрес — закодированные значения, в подпись — по `form`.
/// Принимает: пары, вариант подписи. Возвращает: (строка в адрес, строка в подпись, были ли «непростые» значения).
/// Ошибок нет.
pub fn query_forms(params: &[(&str, String)], form: SignForm) -> (String, String, bool) {
    let mut sorted: Vec<&(&str, String)> = params.iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(b.0));
    let mut special = false;
    let (mut url, mut sig) = (Vec::new(), Vec::new());
    for (k, v) in sorted {
        let enc = percent_encoding::utf8_percent_encode(v, ENC).to_string();
        special |= !value_ok(v);
        url.push(format!("{k}={enc}"));
        sig.push(match form {
            SignForm::Encoded => format!("{k}={enc}"),
            SignForm::Raw => format!("{k}={v}"),
        });
    }
    (url.join("&"), sig.join("&"), special)
}

/// Что делает: значение можно поставить в строку запроса XT — только буквы, цифры и «_ . , -» (см. query).
/// Принимает: значение. Возвращает: да / нет. Ошибок нет.
pub fn value_ok(v: &str) -> bool {
    v.bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b',' | b'-'))
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Символы живого справочника XT 06.11.2026: иероглифы в запрос не ставятся, обычные — ставятся.
    #[test]
    fn value_ok_live_symbols() {
        assert!(value_ok("btc_usdt"));
        assert!(value_ok("1000000hvi_usdt"));
        assert!(!value_ok("牛来_usdt"));
        assert!(!value_ok("龙虾_usdt"));
        assert!(!value_ok("币安底裤_usdt"));
    }

    /// Решение 207: «BNB Smart Chain» — в адрес с %20, в подпись — закодированная или исходная строка.
    #[test]
    fn query_forms_space() {
        let p = vec![
            ("currency", "usdt".to_string()),
            ("chain", "BNB Smart Chain".to_string()),
        ];
        let (url, sig, sp) = query_forms(&p, SignForm::Encoded);
        assert_eq!(url, "chain=BNB%20Smart%20Chain&currency=usdt");
        assert_eq!(sig, url);
        assert!(sp);
        let (_, raw, _) = query_forms(&p, SignForm::Raw);
        assert_eq!(raw, "chain=BNB Smart Chain&currency=usdt");
        let plain = vec![("symbol", "btc_usdt".to_string())];
        let (u, s, sp) = query_forms(&plain, SignForm::Raw);
        assert_eq!(
            (u.as_str(), s.as_str(), sp),
            ("symbol=btc_usdt", "symbol=btc_usdt", false)
        );
    }
}

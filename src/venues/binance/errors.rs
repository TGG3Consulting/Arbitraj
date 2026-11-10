//! Файл: venues/binance/errors.rs
//! Что делает: перевод ошибок Binance (HTTP-статус и поле code ответа) в единые коды шлюза.
//! Кто вызывает: reply.rs, rest.rs и файлы адаптера.
//! От чего зависит: gateway/errors.rs.
//! Настройки: нет.
//!
//! Коды — по официальной таблице https://developers.binance.com/legacy-docs/binance-spot-api-docs/errors (формат
//! {"code": -1121, "msg": "Invalid symbol."}): −1003 TOO_MANY_REQUESTS, −1006 UNEXPECTED_RESP и −1007 TIMEOUT
//! («execution status unknown»), −1008 SERVER_BUSY, −1013 (фильтры), −1015 TOO_MANY_ORDERS, −1021 INVALID_TIMESTAMP,
//! −1022 INVALID_SIGNATURE, −1002 UNAUTHORIZED, −11xx — параметры, −2010 NEW_ORDER_REJECTED (текст в msg: «Account has
//! insufficient balance…», «Filter failure: …», «Market is closed.»), −2011 CANCEL_REJECTED, −2013 NO_SUCH_ORDER,
//! −2014 BAD_API_KEY_FMT, −2015 REJECTED_MBX_KEY («API key, IP, or permissions»). Коды Wallet −4xxx (вывод) — по
//! разделу Error Code Wallet SAPI (−4021 кратность суммы, −4035/−4036 белый список и новый адрес, −4060 депозит в
//! подтверждении, −4106 memo): у вывода — отказ. HTTP — rest-api «LIMITS»: 429 — лимит, 418 — бан IP, 403 — WAF,
//! 5XX — «execution status is UNKNOWN». Текст причины — из msg ответа как есть.

use crate::gateway::errors::{ErrorCode, GatewayError};

use super::ctx::ID;

/// Для какой операции пришла ошибка: от этого зависит единый код у неизвестных кодов.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    /// Ордера.
    Trade,
    /// Вывод.
    Withdraw,
    /// Перевод между своими счетами Binance (POST /sapi/v1/asset/transfer): деньги двигаются — как вывод.
    Move,
    /// Всё остальное (справочники, балансы, проверки, запрос и отмена ордера).
    Other,
}

/// Ошибка по полю code ответа. Принимает: код, msg, операцию, сырой ответ. Возвращает: GatewayError.
pub fn from_code(code: i64, msg: &str, op: Op, raw: &str) -> GatewayError {
    let unified = match code {
        -1022 | -2014 => ErrorCode::InvalidCredentials,
        // −2015 «Invalid API-key, IP, or permissions for action»: ключ, IP или права — точная причина в msg.
        -2015 | -1002 => ErrorCode::InsufficientPermissions,
        -1021 => return clock(code, msg, op, raw),
        -1003 | -1015 => ErrorCode::RateLimited,
        // Итог неизвестен (−1006, −1007 — «execution status unknown»): у ордера и вывода — проверка по своему id.
        -1006 | -1007 if op != Op::Other => ErrorCode::Timeout,
        -1000 | -1001 | -1006 | -1007 | -1008 | -1016 => ErrorCode::Unreachable,
        -2010 if msg.contains("insufficient balance") => ErrorCode::InsufficientBalance,
        // Решение 166: −4060 CAPITAL_WITHDRAW_USER_ASSET_LOCK_DEPOSIT «As your deposit has not reached the required block
        // confirmations, we have temporarily locked {0} asset» (Wallet → Error Code,
        // https://developers.binance.com/en/dev-docs/products/wallet/error-code) — временно заблокировано для вывода.
        -4060 if op == Op::Withdraw => ErrorCode::WithdrawLocked,
        // −2011 / −2013: ордера нет (или не отменён) — не отказ денег, вызывающий решает по смыслу.
        -2011 | -2013 => ErrorCode::BadResponse,
        _ => rejected(op),
    };
    let msg = if msg.is_empty() { "(без msg)" } else { msg };
    GatewayError::new(
        unified,
        Some(venue()),
        format!("Binance, код {code}: {msg}"),
    )
    .with_raw(raw)
}

/// Решение 251: код Binance (поле code) из сырого ответа ошибки ({"code": −4026, "msg": …}). Возвращает: None — сырого
/// ответа нет или в нём нет числа code. Ошибок нет.
pub fn code_of(e: &GatewayError) -> Option<i64> {
    let v: serde_json::Value = serde_json::from_str(e.raw.as_deref()?).ok()?;
    v.get("code")?.as_i64()
}

/// Код отказа по операции: ордер — OrderRejected, вывод — WithdrawRejected, остальное — BadResponse. Ошибок нет.
fn rejected(op: Op) -> ErrorCode {
    match op {
        Op::Trade => ErrorCode::OrderRejected,
        Op::Withdraw => ErrorCode::WithdrawRejected,
        Op::Move | Op::Other => ErrorCode::BadResponse,
    }
}

/// −1021 «Timestamp for this request is outside of the recvWindow»: свой текст про часы сервера. Запрос отклонён —
/// у ордера и вывода отказ, не «итог неизвестен». Ошибок нет.
fn clock(code: i64, msg: &str, op: Op, raw: &str) -> GatewayError {
    let text = format!(
        "Binance, код {code}: время запроса вне recvWindow — проверьте часы сервера (синхронизация времени) ({msg})"
    );
    GatewayError::new(rejected(op), Some(venue()), text).with_raw(raw)
}

/// Ордер, вывод или перевод: запрос мог дойти, а ясного итога нет — «результат неизвестен» (OutcomeUnknown,
/// решение 64): ядро такую операцию не повторяет. Остальное — как есть. Ошибок нет.
pub fn unsure(e: GatewayError, op: Op) -> GatewayError {
    if op == Op::Other || !matches!(e.code, ErrorCode::Timeout | ErrorCode::BadResponse) {
        return e;
    }
    unknown(e, op)
}

/// Деньги могли двинуться → «результат неизвестен» с пояснением. Ошибок нет.
fn unknown(e: GatewayError, op: Op) -> GatewayError {
    let what = match op {
        Op::Trade => "ордер",
        Op::Move => "перевод между счетами",
        _ => "вывод",
    };
    GatewayError {
        code: ErrorCode::OutcomeUnknown,
        message: format!("{} — {what} мог быть принят, итог не получен", e.message),
        ..e
    }
}

/// Ошибка по HTTP-статусу (rest-api «LIMITS» и «HTTP Return Codes»). Принимает: статус, сырой ответ, Retry-After,
/// пояснение паузы. Возвращает: GatewayError.
pub fn from_http(status: u16, raw: &str, retry_after_s: Option<u64>, pause: &str) -> GatewayError {
    let (code, text) = match status {
        401 => (ErrorCode::InvalidCredentials, "неверный ключ (HTTP 401)"),
        403 => (
            ErrorCode::InsufficientPermissions,
            "WAF Binance отклонил запрос (HTTP 403)",
        ),
        418 => (
            ErrorCode::RateLimited,
            "IP заблокирован за продолжение после 429 (HTTP 418, от 2 минут до 3 суток) — запросы прекращены",
        ),
        429 => (
            ErrorCode::RateLimited,
            "превышен лимит запросов (HTTP 429) — продолжение даст бан IP (418)",
        ),
        500..=599 => (
            ErrorCode::Timeout,
            "ошибка на стороне Binance (HTTP 5XX) — итог операции неизвестен, нужна проверка",
        ),
        _ => (ErrorCode::BadResponse, "непонятный ответ"),
    };
    let wait = retry_after_s
        .map(|s| format!(", Retry-After {s} с"))
        .unwrap_or_default();
    let pause = if pause.is_empty() {
        String::new()
    } else {
        format!("; {pause}")
    };
    GatewayError::new(
        code,
        Some(venue()),
        format!("Binance: {text}, HTTP {status}{wait}{pause}"),
    )
    .with_raw(raw)
}

/// Сбой HTTP-обмена: нет связи (Unreachable), запрос мог дойти без ответа (Timeout), тело не текст или больше
/// предела (BadResponse). Принимает: сбой, сроки. Возвращает: GatewayError.
pub fn from_fail(
    f: &crate::gateway::http::Fail,
    lim: &crate::gateway::http::Limits,
) -> GatewayError {
    use crate::gateway::http::Fail;
    let code = match f {
        Fail::Connect(_) => ErrorCode::Unreachable,
        Fail::NotText | Fail::TooBig { .. } => ErrorCode::BadResponse,
        _ => ErrorCode::Timeout,
    };
    GatewayError::new(code, Some(venue()), format!("Binance: {}", f.text(lim)))
}

/// Сбой обмена по операции с деньгами: любой сбой, кроме «соединение не установлено», — «результат неизвестен».
/// Принимает: сбой, сроки, операцию. Возвращает: GatewayError.
pub fn fail_for(
    f: &crate::gateway::http::Fail,
    lim: &crate::gateway::http::Limits,
    op: Op,
) -> GatewayError {
    let e = from_fail(f, lim);
    if op == Op::Other || f.not_sent() {
        return e;
    }
    unknown(e, op)
}

/// Ответ не удалось разобрать. Принимает: что не так, сырой ответ. Возвращает: BadResponse.
pub fn bad(what: &str, raw: &str) -> GatewayError {
    GatewayError::new(
        ErrorCode::BadResponse,
        Some(venue()),
        format!("Binance: непонятный ответ: {what}"),
    )
    .with_raw(raw)
}

/// Площадка Binance для ошибок. Ошибок нет.
fn venue() -> crate::core::types::VenueId {
    crate::core::types::VenueId(ID.to_string())
}

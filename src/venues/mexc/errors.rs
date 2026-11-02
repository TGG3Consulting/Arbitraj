//! Файл: venues/mexc/errors.rs
//! Что делает: перевод ошибок MEXC (HTTP-статус и поле code ответа) в единые коды шлюза.
//! Кто вызывает: venues/mexc/rest.rs и файлы адаптера.
//! От чего зависит: gateway/errors.rs.
//! Настройки: нет.
//!
//! Коды — по официальной таблице «Error Code» https://www.mexc.com/api-docs/spot-v3/error-code и разделу «HTTP Return
//! Codes» https://www.mexc.com/api-docs/spot-v3/introduction (403 — WAF, 429 — превышен лимит, «the IP may be banned
//! soon», 418/429 — заголовок Retry-After, 5XX — «the execution status is UNKNOWN and could have been a success»).
//! Текст причины всегда берётся из поля msg ответа MEXC — он виден Тиграну как есть.

use crate::gateway::errors::{ErrorCode, GatewayError};

use super::ctx::ID;

/// Для какой операции пришла ошибка: от этого зависит единый код у неизвестных кодов MEXC.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    /// Ордера.
    Trade,
    /// Вывод.
    Withdraw,
    /// Перевод между своими счетами MEXC (POST /api/v3/capital/transfer): деньги двигаются — как вывод.
    Move,
    /// Всё остальное (справочники, балансы, проверки, отмена и запрос ордера).
    Other,
}

/// Ошибка по полю code ответа MEXC. Принимает: код, текст msg, операцию, сырой ответ. Возвращает: GatewayError.
pub fn from_code(code: i64, msg: &str, op: Op, raw: &str) -> GatewayError {
    let unified = match code {
        // Ключ, подпись, время, права, IP (Error Code: 10072, 700001, 700002, 602, 700003, 700006, 700007, 401).
        10072 | 700001 | 700002 | 602 => ErrorCode::InvalidCredentials,
        700003 => ErrorCode::InvalidCredentials,
        700007 | 401 | 30020 | 70011 => ErrorCode::InsufficientPermissions,
        700006 => ErrorCode::IpNotAllowed,
        // Баланс (10101 «does not have enough available balance», 30004 «Insufficient position», 30005 «Oversold»).
        10101 | 30004 | 30005 => ErrorCode::InsufficientBalance,
        // Сервер занят: для ордера и вывода итог неизвестен (Timeout — нужна проверка), для остального — недоступно.
        500 | 503 | 504 | 20002 | 11444 if op != Op::Other => ErrorCode::Timeout,
        500 | 503 | 504 | 20002 | 11444 => ErrorCode::Unreachable,
        // Ордер не найден (-2011 «Unknown order sent»).
        -2011 => ErrorCode::BadResponse,
        _ => match op {
            Op::Trade => ErrorCode::OrderRejected,
            Op::Withdraw => ErrorCode::WithdrawRejected,
            Op::Move | Op::Other => ErrorCode::BadResponse,
        },
    };
    let text = format!("MEXC, код {code}: {msg}");
    GatewayError::new(unified, Some(venue()), text).with_raw(raw)
}

/// Ордер, вывод или перевод: запрос мог дойти, а ясного итога нет — «результат неизвестен» (OutcomeUnknown,
/// решение 64): ядро такую операцию не повторяет. Остальные ошибки и Op::Other — как есть. Ошибок нет.
pub fn unsure(e: GatewayError, op: Op) -> GatewayError {
    if op == Op::Other || !matches!(e.code, ErrorCode::Timeout | ErrorCode::BadResponse) {
        return e;
    }
    unknown(e, op)
}

/// Деньги могли двинуться → «результат неизвестен» с пояснением. Принимает: ошибку, операцию. Ошибок нет.
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

/// Ошибка по HTTP-статусу (когда тело не в формате MEXC). 403 — WAF; 418/429 — лимит (дальше — бан IP от 2 минут
/// до 3 суток: «Continuing to exceed limits after receiving 429 will result in an IP ban»); 5XX — сторона MEXC.
pub fn from_http(status: u16, raw: &str) -> GatewayError {
    let (code, text) = match status {
        401 => (ErrorCode::InvalidCredentials, "неверный ключ (HTTP 401)"),
        403 => (
            ErrorCode::InsufficientPermissions,
            "WAF MEXC отклонил запрос (HTTP 403)",
        ),
        418 => (
            ErrorCode::RateLimited,
            "IP заблокирован за превышение лимита (HTTP 418) — прекратить запросы, ждать Retry-After",
        ),
        429 => (
            ErrorCode::RateLimited,
            "превышен лимит запросов (HTTP 429) — дальше бан IP; ждать Retry-After",
        ),
        500..=599 => (
            ErrorCode::Timeout,
            "ошибка на стороне MEXC (HTTP 5XX) — итог операции неизвестен, нужна проверка",
        ),
        _ => (ErrorCode::BadResponse, "непонятный ответ"),
    };
    GatewayError::new(code, Some(venue()), format!("MEXC: {text}, HTTP {status}")).with_raw(raw)
}

/// Сбой HTTP-обмена: нет связи (Unreachable), запрос мог дойти, а ответа нет (Timeout), тело не текст или больше
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
    GatewayError::new(code, Some(venue()), format!("MEXC: {}", f.text(lim)))
}

/// Сбой обмена по операции с деньгами (Trade, Withdraw, Move): любой сбой, кроме «соединение не установлено», —
/// «результат неизвестен». Принимает: сбой, сроки, операцию. Возвращает: GatewayError. Ошибок нет.
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

/// Ответ не удалось разобрать.
pub fn bad(what: &str, raw: &str) -> GatewayError {
    GatewayError::new(
        ErrorCode::BadResponse,
        Some(venue()),
        format!("MEXC: непонятный ответ: {what}"),
    )
    .with_raw(raw)
}

/// Площадка MEXC для ошибок.
fn venue() -> crate::core::types::VenueId {
    crate::core::types::VenueId(ID.to_string())
}

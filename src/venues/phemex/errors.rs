//! Файл: venues/phemex/errors.rs
//! Что делает: перевод ошибок Phemex (HTTP-статус и поле code ответа) в единые коды шлюза.
//! Кто вызывает: reply.rs, rest.rs и файлы адаптера.
//! От чего зависит: gateway/errors.rs, gateway/http.rs.
//! Настройки: нет.
//!
//! Официально (General API information, https://phemex-docs.github.io/#http-return-codes и #error-codes):
//! - HTTP: «401 … unauthenticated», «403 … lack of priviledge», «429 … breaking a request ratelimit», «5XX … Phemex
//!   internal errors. Note: This doesn't means the operation failed, the execution status is UNKNOWN and could be
//!   Succeed» — у ордера, вывода и перевода 5XX = «итог неизвестен» (решение 64).
//! - Тело: code 0 — успех, иначе ошибка, msg — «short error description». Таблица BizError: 19999
//!   REQUEST_IS_DUPLICATED, 10001 OM_DUPLICATE_ORDERID, 11085 TE_CL_ORD_ID_DUPLICATE, 11033 TE_ORDER_ID_DUPLICATE —
//!   дубль; 11001 TE_NO_ENOUGH_AVAILABLE_BALANCE, 11100 TE_WALLET_INSUFFICIENT_BALANCE, 11101, 11105
//!   TE_PLACE_ORDER_INSUFFICIENT_BASE_BALANCE, 11106 …QUOTE_BALANCE — не хватает средств; 11053/11055 цена, 11056/11058
//!   количество, 11114/11115 сумма — отказ ордера. Кодов «ключ не найден», «счёт не открыт», «адрес не в белом списке»
//!   в документации нет: они переводятся по HTTP-статусу и операции, текст — msg как есть.

use crate::gateway::errors::{ErrorCode, GatewayError};

use super::ctx::ID;

/// Для какой операции пришла ошибка: от неё зависит единый код у отказов и «итог неизвестен».
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    /// Ордера.
    Trade,
    /// Вывод.
    Withdraw,
    /// Перевод между своими счетами Phemex (CONTRACT → SPOT): деньги двигаются — как вывод.
    Move,
    /// Всё остальное (справочники, балансы, проверки, запрос и отмена ордера).
    Other,
}

/// Коды «не хватает средств» из таблицы BizError.
const NO_FUNDS: [&str; 5] = ["11001", "11100", "11101", "11105", "11106"];

/// Ошибка по полю code ответа и HTTP-статусу. Принимает: статус, код (текст), msg, операцию, сырой ответ.
/// Возвращает: GatewayError.
pub fn from_code(status: u16, code: &str, msg: &str, op: Op, raw: &str) -> GatewayError {
    let unified = match (status, code) {
        (429, _) => ErrorCode::RateLimited,
        (401, _) => ErrorCode::InvalidCredentials,
        (403, _) => ErrorCode::InsufficientPermissions,
        (500..=599, _) if op != Op::Other => ErrorCode::Timeout,
        (500..=599, _) => ErrorCode::Unreachable,
        (_, c) if NO_FUNDS.contains(&c) => ErrorCode::InsufficientBalance,
        _ => rejected(op),
    };
    let msg = if msg.is_empty() { "(без msg)" } else { msg };
    GatewayError::new(
        unified,
        Some(venue()),
        format!("Phemex, код {code} (HTTP {status}): {msg}"),
    )
    .with_raw(raw)
}

/// Отказ по операции: ордер — OrderRejected, вывод — WithdrawRejected, остальное — BadResponse. Ошибок нет.
pub fn rejected(op: Op) -> ErrorCode {
    match op {
        Op::Trade => ErrorCode::OrderRejected,
        Op::Withdraw => ErrorCode::WithdrawRejected,
        Op::Move | Op::Other => ErrorCode::BadResponse,
    }
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

/// Ошибка по HTTP-статусу (тело без понятного code). Принимает: статус, операцию, сырой ответ. Возвращает: GatewayError.
pub fn from_http(status: u16, op: Op, raw: &str) -> GatewayError {
    let (code, text) = match status {
        401 => (ErrorCode::InvalidCredentials, "ключ не принят (HTTP 401)"),
        403 => (
            ErrorCode::InsufficientPermissions,
            "нет прав у ключа (HTTP 403)",
        ),
        429 => (ErrorCode::RateLimited, "превышен лимит запросов (HTTP 429)"),
        500..=599 => (
            ErrorCode::Timeout,
            "ошибка на стороне Phemex (HTTP 5XX) — итог операции неизвестен",
        ),
        400 => (rejected(op), "запрос не принят (HTTP 400)"),
        _ => (ErrorCode::BadResponse, "непонятный ответ"),
    };
    GatewayError::new(
        code,
        Some(venue()),
        format!("Phemex: {text}, HTTP {status}"),
    )
    .with_raw(raw)
}

/// Сбой HTTP-обмена: нет связи (Unreachable), ответа нет (Timeout), тело не текст или велико (BadResponse).
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
    GatewayError::new(code, Some(venue()), format!("Phemex: {}", f.text(lim)))
}

/// Сбой обмена по операции с деньгами: всё, кроме «соединение не установлено», — «результат неизвестен».
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
        format!("Phemex: непонятный ответ: {what}"),
    )
    .with_raw(raw)
}

/// Площадка Phemex для ошибок.
fn venue() -> crate::core::types::VenueId {
    crate::core::types::VenueId(ID.to_string())
}

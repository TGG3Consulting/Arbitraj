//! Файл: venues/gate/errors.rs
//! Что делает: перевод ошибок Gate (HTTP-статус и поле label ответа) в единые коды шлюза.
//! Кто вызывает: venues/gate/reply.rs, rest.rs и файлы адаптера.
//! От чего зависит: gateway/errors.rs.
//! Настройки: нет.
//!
//! Официальная документация, API v4 → «Error Response» и «Error label list» https://www.gate.com/docs/developers/apiv4/en/
//! : ответ-ошибка — статус не 2xx и тело {label, message} (иногда detail); «Error handling mechanism with this field
//! [message] is highly NOT recommended» — решение принимается только по label, message показывается Тиграну как есть.
//! Группы меток: аутентификация (INVALID_CREDENTIALS, INVALID_KEY, IP_FORBIDDEN, READ_ONLY, INVALID_SIGNATURE,
//! MISSING_REQUIRED_HEADER, REQUEST_EXPIRED — «Request Timestamp is far from the server time», ACCOUNT_LOCKED, FORBIDDEN),
//! кошелёк (ADDRESS_NOT_USED, TOO_FAST, WITHDRAWAL_OVER_LIMIT, API_WITHDRAW_DISABLED, DUPLICATE_REQUEST, ORDER_EXISTS,
//! INVALID_CLIENT_ORDER_ID…), спот (INVALID_PRECISION, INVALID_CURRENCY, INVALID_CURRENCY_PAIR, ORDER_NOT_FOUND,
//! ORDER_CLOSED, ORDER_CANCELLED, QUANTITY_NOT_ENOUGH, BALANCE_NOT_ENOUGH, AMOUNT_TOO_LITTLE, AMOUNT_TOO_MUCH,
//! REPEATED_CREATION…), сервер (INTERNAL, SERVER_ERROR, INTERNAL_SERVER_ERROR, TOO_BUSY). Статусы: 429 — «Too many
//! requests» (TOO_MANY_REQUESTS); 5xx — «critical server error during processing» (итог ордера и вывода неизвестен).

use crate::gateway::errors::{ErrorCode, GatewayError};

use super::ctx::ID;

/// Для какой операции пришла ошибка: от этого зависит единый код у неизвестных меток Gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    /// Ордера (постановка и отмена).
    Trade,
    /// Вывод.
    Withdraw,
    /// Всё остальное (справочники, балансы, проверки, запрос ордера).
    Other,
}

/// Что делает: ошибка по метке label ответа Gate.
/// Принимает: HTTP-статус, метку, текст message, операцию, сырой ответ. Возвращает: GatewayError.
pub fn from_label(status: u16, label: &str, msg: &str, op: Op, raw: &str) -> GatewayError {
    let unified = match label {
        "INVALID_CREDENTIALS" | "INVALID_KEY" | "INVALID_SIGNATURE" | "MISSING_REQUIRED_HEADER" => {
            ErrorCode::InvalidCredentials
        }
        // Время запроса далеко от времени сервера (окно — 60 с): ключ ни при чём, свой текст про часы.
        "REQUEST_EXPIRED" => return clock(msg, op, raw),
        "IP_FORBIDDEN" => ErrorCode::IpNotAllowed,
        "READ_ONLY" | "FORBIDDEN" | "ACCOUNT_LOCKED" | "API_WITHDRAW_DISABLED" => {
            ErrorCode::InsufficientPermissions
        }
        // Решение 179 (живой случай 05.11, сделка 20261105151153495ACH): у вывода TOO_FAST — «Withdrawal frequency is
        // limited to 10s», вывод не принят — повтор того же вывода (core/transfer_locked.rs, transfer.locked_retry_ms
        // по площадке gate не меньше 10000), как «временно нельзя вывести».
        "TOO_FAST" if op == Op::Withdraw => ErrorCode::WithdrawLocked,
        "TOO_MANY_REQUESTS" | "TOO_FAST" => ErrorCode::RateLimited,
        "BALANCE_NOT_ENOUGH" | "QUANTITY_NOT_ENOUGH" => ErrorCode::InsufficientBalance,
        // Адрес вывода не из списка кабинета: «ADDRESS_NOT_USED — address not used in web console».
        "ADDRESS_NOT_USED" => ErrorCode::InvalidAddress,
        // Решение 253, дополнение t82 (Тигран 10.11): Error label list → Wallet (сверено 11.11.2026) — ACCOUNT_EXCEPTION
        // «Abnormal account» у вывода — аккаунт (AccountRestricted). WITHDRAWAL_OVER_LIMIT «Withdrawal limit exceeded» —
        // чей лимит (аккаунта за сутки или суммы монеты), из таблицы не видно: как раньше (вопрос в docs/TASKS.md).
        "ACCOUNT_EXCEPTION" if op == Op::Withdraw => ErrorCode::AccountRestricted,
        // Сервер занят или ошибка сервера: у ордера и вывода итог неизвестен (Timeout → «результат неизвестен»).
        "INTERNAL" | "SERVER_ERROR" | "INTERNAL_SERVER_ERROR" | "TOO_BUSY" if op != Op::Other => {
            ErrorCode::Timeout
        }
        "INTERNAL" | "SERVER_ERROR" | "INTERNAL_SERVER_ERROR" | "TOO_BUSY" => {
            ErrorCode::Unreachable
        }
        // Ордер не найден / закрыт — ответ на запрос или отмену (вызывающий решает, что дальше).
        "ORDER_NOT_FOUND" | "ORDER_CLOSED" | "ORDER_CANCELLED" => ErrorCode::BadResponse,
        _ if status == 429 => ErrorCode::RateLimited,
        _ if status >= 500 && op != Op::Other => ErrorCode::Timeout,
        _ if status >= 500 => ErrorCode::Unreachable,
        _ => rejected(op),
    };
    let msg = if msg.is_empty() {
        "(без message)"
    } else {
        msg
    };
    let text = format!("Gate, HTTP {status}, {label}: {msg}");
    GatewayError::new(unified, Some(venue()), text).with_raw(raw)
}

/// Код отказа по операции: ордер — OrderRejected, вывод — WithdrawRejected, остальное — BadResponse. Ошибок нет.
fn rejected(op: Op) -> ErrorCode {
    match op {
        Op::Trade => ErrorCode::OrderRejected,
        Op::Withdraw => ErrorCode::WithdrawRejected,
        Op::Other => ErrorCode::BadResponse,
    }
}

/// REQUEST_EXPIRED: «The gap between the Timestamp value and current time cannot exceed 60 seconds» (Authentication).
/// Запрос отклонён: у ордера и вывода — отказ, у остального — BadResponse. Ошибок нет.
fn clock(msg: &str, op: Op, raw: &str) -> GatewayError {
    let text = format!(
        "Gate, REQUEST_EXPIRED: время запроса далеко от времени сервера (не больше 60 с) — проверьте часы сервера ({msg})"
    );
    GatewayError::new(rejected(op), Some(venue()), text).with_raw(raw)
}

/// Ордер или вывод: запрос мог дойти, а ясного итога нет — «результат неизвестен» (OutcomeUnknown, решение 64): ядро
/// такую операцию не повторяет. Op::Other и остальные коды — как есть. Ошибок нет.
pub fn unsure(e: GatewayError, op: Op) -> GatewayError {
    if op == Op::Other || !matches!(e.code, ErrorCode::Timeout | ErrorCode::BadResponse) {
        return e;
    }
    unknown(e, op)
}

/// Деньги могли двинуться → «результат неизвестен» с пояснением. Ошибок нет.
fn unknown(e: GatewayError, op: Op) -> GatewayError {
    let what = if op == Op::Trade {
        "ордер"
    } else {
        "вывод"
    };
    GatewayError {
        code: ErrorCode::OutcomeUnknown,
        message: format!("{} — {what} мог быть принят, итог не получен", e.message),
        ..e
    }
}

/// Ошибка по HTTP-статусу без метки (тело не {label}). Принимает: статус, сырой ответ, Retry-After. Возвращает: ошибку.
pub fn from_http(status: u16, raw: &str, retry_after_s: Option<u64>) -> GatewayError {
    let (code, text) = match status {
        401 => (ErrorCode::InvalidCredentials, "ключ не принят (HTTP 401)"),
        403 => (
            ErrorCode::InsufficientPermissions,
            "доступ запрещён (HTTP 403)",
        ),
        429 => (ErrorCode::RateLimited, "превышен лимит запросов (HTTP 429)"),
        500..=599 => (
            ErrorCode::Timeout,
            "ошибка на стороне Gate (HTTP 5xx) — итог операции неизвестен",
        ),
        _ => (ErrorCode::BadResponse, "непонятный ответ"),
    };
    let wait = retry_after_s
        .map(|s| format!(", Retry-After {s} с"))
        .unwrap_or_default();
    GatewayError::new(
        code,
        Some(venue()),
        format!("Gate: {text}, HTTP {status}{wait}"),
    )
    .with_raw(raw)
}

/// Сбой HTTP-обмена: нет связи (Unreachable), запрос мог дойти без ответа (Timeout), тело не текст или больше предела
/// (BadResponse). Принимает: сбой, сроки. Возвращает: GatewayError.
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
    GatewayError::new(code, Some(venue()), format!("Gate: {}", f.text(lim)))
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

/// Ответ не удалось разобрать. Принимает: что не так, сырой ответ.
pub fn bad(what: &str, raw: &str) -> GatewayError {
    GatewayError::new(
        ErrorCode::BadResponse,
        Some(venue()),
        format!("Gate: непонятный ответ: {what}"),
    )
    .with_raw(raw)
}

/// Площадка Gate для ошибок.
fn venue() -> crate::core::types::VenueId {
    crate::core::types::VenueId(ID.to_string())
}

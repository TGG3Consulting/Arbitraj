//! Файл: venues/poloniex/errors.rs
//! Что делает: перевод ошибок Poloniex (HTTP-статус и поле code тела) в единые коды шлюза.
//! Кто вызывает: reply.rs, rest.rs и файлы адаптера.
//! От чего зависит: gateway/errors.rs, gateway/http.rs.
//! Настройки: нет.
//!
//! Официально (Spot → Error Codes, https://api-docs.poloniex.com/spot/error-code): 500 «Internal System Error», 603
//! «Internal Request Timeout», 21709 «Insufficient balance», 21601 «Account locked», 21312 «Client orderId already
//! exists», 21301 «Order not found», 21322 / 21330 / 21350 — меньше минимума, 21335–21337 — масштаб числа, 21340 /
//! 21341 — обслуживание / только post-only, 21351–21354 — торговля заморожена, US-клиенты, не подтверждён email, 21356
//! — «too much price movement», 24201 «Service busy». Кодов ключа, подписи, прав, IP и лимита запросов в таблице НЕТ:
//! статусы 400 (метка времени), 408 (recvWindow) — со страницы Authentication; 401 / 403 / 429 — по общему смыслу
//! HTTP (проверить на живых ответах, урок 15). Текст причины — поле message ответа как есть.

use crate::gateway::errors::{ErrorCode, GatewayError};

use super::ctx::ID;

/// Для какой операции пришла ошибка: от неё зависит единый код у отказов и «итог неизвестен».
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    /// Ордера.
    Trade,
    /// Вывод.
    Withdraw,
    /// Всё остальное (справочники, балансы, проверки, запрос и отмена ордера).
    Other,
}

/// Ошибка по полю code ответа и HTTP-статусу. Принимает: статус, код (строка), message, операцию, сырой ответ.
/// Возвращает: GatewayError.
pub fn from_code(status: u16, code: &str, msg: &str, op: Op, raw: &str) -> GatewayError {
    let unified = match (status, code) {
        (429, _) => ErrorCode::RateLimited,
        (401, _) => ErrorCode::InvalidCredentials,
        (403, _) => ErrorCode::InsufficientPermissions,
        (_, "21709") => ErrorCode::InsufficientBalance,
        // Решение 253, дополнение t82 (Тигран 10.11): 21601 «Account locked. Contact support» (Error Codes → Accounts,
        // сверено 11.11.2026) у вывода — аккаунт (AccountRestricted). 21711 «Currency locked» — монета, как раньше.
        (_, "21601") if op == Op::Withdraw => ErrorCode::AccountRestricted,
        // Сервер занят или внутренняя ошибка: у денег итог неизвестен.
        (_, "500" | "603" | "415" | "24201") | (500..=599, _) if op != Op::Other => {
            ErrorCode::Timeout
        }
        (_, "500" | "603" | "415" | "24201") | (500..=599, _) => ErrorCode::Unreachable,
        _ => rejected(op),
    };
    let msg = if msg.is_empty() {
        "(без message)"
    } else {
        msg
    };
    GatewayError::new(
        unified,
        Some(venue()),
        format!("Poloniex, код {code}: {msg} (HTTP {status})"),
    )
    .with_raw(raw)
}

/// Отказ по операции: ордер — OrderRejected, вывод — WithdrawRejected, остальное — BadResponse. Ошибок нет.
pub fn rejected(op: Op) -> ErrorCode {
    match op {
        Op::Trade => ErrorCode::OrderRejected,
        Op::Withdraw => ErrorCode::WithdrawRejected,
        Op::Other => ErrorCode::BadResponse,
    }
}

/// Ордер или вывод: запрос мог дойти, а ясного итога нет — «результат неизвестен» (OutcomeUnknown, решение 64):
/// ядро такую операцию не повторяет. Остальное — как есть. Ошибок нет.
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

/// Ошибка по HTTP-статусу (тело без понятного code). Принимает: статус, операцию, сырой ответ. Возвращает: GatewayError.
pub fn from_http(status: u16, op: Op, raw: &str) -> GatewayError {
    let (code, text) = match status {
        400 => (
            rejected(op),
            "запрос не принят (HTTP 400; по Authentication — и метка времени дальше 1 с от часов биржи)",
        ),
        401 => (ErrorCode::InvalidCredentials, "ключ не принят (HTTP 401)"),
        403 => (ErrorCode::InsufficientPermissions, "нет доступа (HTTP 403)"),
        408 => (rejected(op), "вышли за recvWindow (HTTP 408)"),
        429 => (ErrorCode::RateLimited, "превышен лимит запросов (HTTP 429)"),
        500..=599 => (
            ErrorCode::Timeout,
            "ошибка на стороне Poloniex (HTTP 5XX) — итог операции неизвестен",
        ),
        _ => (ErrorCode::BadResponse, "непонятный ответ"),
    };
    GatewayError::new(
        code,
        Some(venue()),
        format!("Poloniex: {text}, HTTP {status}"),
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
    GatewayError::new(code, Some(venue()), format!("Poloniex: {}", f.text(lim)))
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
        format!("Poloniex: непонятный ответ: {what}"),
    )
    .with_raw(raw)
}

/// Площадка Poloniex для ошибок.
fn venue() -> crate::core::types::VenueId {
    crate::core::types::VenueId(ID.to_string())
}

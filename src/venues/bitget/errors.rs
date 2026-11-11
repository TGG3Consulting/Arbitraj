//! Файл: venues/bitget/errors.rs
//! Что делает: перевод ошибок Bitget (HTTP-статус и поле code ответа) в единые коды шлюза.
//! Кто вызывает: reply.rs, rest.rs и файлы адаптера.
//! От чего зависит: gateway/errors.rs, gateway/http.rs.
//! Настройки: нет.
//!
//! Официально:
//! - HTTP-статусы (Classic → REST API, https://www.bitget.com/docs/classic/rest-api): 400 «Bad Request – Invalid request
//!   format», 401 «Invalid API Key», 403 «You do not have access to the requested resource», 404, 429 «Too many requests,
//!   your requests have been limited», 500 «We had a problem with our server».
//! - Коды тела (https://www.bitget.com/docs/api/error-code — страница помечена «Demo error codes», в ней 7 кодов): 429
//!   «Too many requests», 00001, 00171, 00172, 01001, 01002 (проверка параметров), 11000 «Withdrawal whitelist is
//!   enabled; your account can only withdraw to address-book addresses»; 43118 — дубль clientOid (примеры batch-orders,
//!   раздел Spot → Trade); 40938 — адресная книга вывода, в таблице нет, по живому ответу (решение 220). Полного перечня кодов в документации нет: остальные коды переводятся по HTTP-статусу ответа,
//!   а без него — отказом по операции; текст причины — поле msg ответа Bitget как есть.

use crate::gateway::errors::{ErrorCode, GatewayError};

use super::ctx::ID;

/// Для какой операции пришла ошибка: от неё зависит единый код у отказов и «итог неизвестен».
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    /// Ордера.
    Trade,
    /// Вывод.
    Withdraw,
    /// Перевод между своими счетами Bitget (spot ↔ p2p/funding): деньги двигаются — как вывод.
    Move,
    /// Всё остальное (справочники, балансы, проверки, запрос и отмена ордера).
    Other,
}

/// Ошибка по полю code ответа и HTTP-статусу. Принимает: статус, код (строка), msg, операцию, сырой ответ.
/// Возвращает: GatewayError.
pub fn from_code(status: u16, code: &str, msg: &str, op: Op, raw: &str) -> GatewayError {
    let unified = match (status, code) {
        (429, _) | (_, "429") => ErrorCode::RateLimited,
        (401, _) => ErrorCode::InvalidCredentials,
        (403, _) => ErrorCode::InsufficientPermissions,
        // Белый список адресов вывода включён — адрес не в адресной книге (урок 19). Решение 220: 40938 «Your account
        // can only withdraw to addresses in the address book» (HTTP 400) — тот же смысл, что 11000. В официальной
        // таблице кодов (https://www.bitget.com/docs/api/error-code) его НЕТ; основание — живой ответ Bitget на вывод
        // (сделки CHEEMS, AVA, RWA; tests/data/bitget_40938_2026-11-07). Решение 253, дополнение t82 (Тигран 10.11):
        // адрес / белый список — InvalidAddress (монета не закрывается, алерт «проблема в аккаунте»; было
        // WithdrawRejected) при любой операции.
        (_, "11000" | "40938") => ErrorCode::InvalidAddress,
        // Сервер: у денег итог неизвестен.
        (500..=599, _) if op != Op::Other => ErrorCode::Timeout,
        (500..=599, _) => ErrorCode::Unreachable,
        _ => rejected(op),
    };
    let msg = if msg.is_empty() { "(без msg)" } else { msg };
    GatewayError::new(
        unified,
        Some(venue()),
        format!("Bitget, код {code} (HTTP {status}): {msg}"),
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
            "нет доступа к ресурсу (HTTP 403)",
        ),
        429 => (ErrorCode::RateLimited, "превышен лимит запросов (HTTP 429)"),
        500..=599 => (
            ErrorCode::Timeout,
            "ошибка на стороне Bitget (HTTP 5XX) — итог операции неизвестен",
        ),
        400 => (rejected(op), "запрос не принят (HTTP 400)"),
        _ => (ErrorCode::BadResponse, "непонятный ответ"),
    };
    GatewayError::new(
        code,
        Some(venue()),
        format!("Bitget: {text}, HTTP {status}"),
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
    GatewayError::new(code, Some(venue()), format!("Bitget: {}", f.text(lim)))
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
        format!("Bitget: непонятный ответ: {what}"),
    )
    .with_raw(raw)
}

/// Площадка Bitget для ошибок.
fn venue() -> crate::core::types::VenueId {
    crate::core::types::VenueId(ID.to_string())
}

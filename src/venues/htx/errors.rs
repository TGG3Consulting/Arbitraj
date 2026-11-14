//! Файл: venues/htx/errors.rs
//! Что делает: перевод ошибок HTX (HTTP-статус, err-code / err_code ответа v1 и code ответа v2) в единые коды шлюза.
//! Текст ошибки — «HTX, код <код>: <сообщение>» (так его разбирает notify/reason.rs, решение 164).
//! Кто вызывает: reply.rs, rest.rs и файлы адаптера.
//! От чего зависит: gateway/errors.rs, gateway/http.rs.
//! Настройки: нет.
//!
//! Официально (https://huobiapi.github.io/docs/spot/v1/en/):
//! - «Trading → Error Code»: login-required («signature missing, or user and key do not match»),
//!   api-signature-not-valid (пример «API Access → Response Format»), order-accountbalance-error,
//!   order-value-min-error, order-orderprice-precision-error, order-orderamount-precision-error, order-invalid-price,
//!   order-type-invalid, invalid-client-order-id («duplicate within the last 24h»), base-symbol-trade-disabled,
//!   order-disabled, base-system-error («Often a cache issue; retry later» — итог операции с деньгами неизвестен),
//!   base-record-invalid / not-found / base-not-found (запись не найдена).
//! - «Wallet → Error Code» (v2): 200 success, 500 «Server internal error», 1002 unauthorized, 1003 «invalid signature»,
//!   2002 «invalid field value», 2003 «missing Required field».
//! - Ответа на превышение лимита REST в документации нет («API Access → New Version Rate limit Rule» описывает только
//!   заголовки X-HB-RateLimit-*), поэтому «слишком часто» — только HTTP 429 (общий смысл кода HTTP, RFC 6585); точный
//!   ответ — проверить на живом ответе. Остальные коды — отказ по операции, текст — сообщение HTX как есть.

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

/// Коды «ключ или подпись не приняты».
const AUTH: [&str; 4] = ["login-required", "api-signature-not-valid", "1002", "1003"];
/// Коды «не хватает средств».
const BALANCE: [&str; 1] = ["order-accountbalance-error"];
/// Коды «система занята, повторить позже» — у денег итог неизвестен.
const BUSY: [&str; 2] = ["base-system-error", "500"];

/// Ошибка по коду ответа и HTTP-статусу. Принимает: статус, код (текст), сообщение, операцию, сырой ответ.
/// Возвращает: GatewayError.
pub fn from_code(status: u16, code: &str, msg: &str, op: Op, raw: &str) -> GatewayError {
    let unified = match (status, code) {
        (429, _) => ErrorCode::RateLimited,
        (401, _) => ErrorCode::InvalidCredentials,
        (403, _) => ErrorCode::InsufficientPermissions,
        _ if AUTH.contains(&code) => ErrorCode::InvalidCredentials,
        _ if BALANCE.contains(&code) => ErrorCode::InsufficientBalance,
        (500..=599, _) => busy(op),
        _ if BUSY.contains(&code) => busy(op),
        // Решение 268 (Тигран 14.11), Error Code (https://huobiapi.github.io/docs/spot/v1/en/): order-limitorder-price-min-error
        // «Sell price cannot be lower than specific price…», order-limitorder-price-max-error «Buy price cannot be higher
        // than specific price…» — цена лимитного ордера вне диапазона биржи (PriceOutOfRange). order-price-greater-than-limit
        // / order-price-less-than-limit («before market opens») и order-invalid-price сюда НЕ входят. Только у операции
        // ордера; рыночная продажа код снимает (market_sell.rs).
        // Дополнение 268 (ревью 2): только у операции ордера (Op::Trade); у прочих операций — как до решения 268.
        (_, "order-limitorder-price-min-error" | "order-limitorder-price-max-error")
            if op == Op::Trade =>
        {
            ErrorCode::PriceOutOfRange
        }
        _ => rejected(op),
    };
    let msg = if msg.is_empty() {
        "(без сообщения)"
    } else {
        msg
    };
    GatewayError::new(unified, Some(venue()), format!("HTX, код {code}: {msg}")).with_raw(raw)
}

/// Сервер занят: для денег — Timeout (итог неизвестен, unsure → OutcomeUnknown), иначе Unreachable. Ошибок нет.
fn busy(op: Op) -> ErrorCode {
    if op == Op::Other {
        ErrorCode::Unreachable
    } else {
        ErrorCode::Timeout
    }
}

/// Отказ по операции: ордер — OrderRejected, вывод — WithdrawRejected, остальное — BadResponse. Ошибок нет.
pub fn rejected(op: Op) -> ErrorCode {
    match op {
        Op::Trade => ErrorCode::OrderRejected,
        Op::Withdraw => ErrorCode::WithdrawRejected,
        Op::Other => ErrorCode::BadResponse,
    }
}

/// Ордер или вывод: запрос мог дойти, а ясного итога нет — «результат неизвестен» (OutcomeUnknown, решение 64): ядро
/// такую операцию не повторяет. Остальное — как есть. Ошибок нет.
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

/// Ошибка по HTTP-статусу (тело без понятного кода). Принимает: статус, операцию, сырой ответ. Возвращает: GatewayError.
pub fn from_http(status: u16, op: Op, raw: &str) -> GatewayError {
    let (code, text) = match status {
        401 => (ErrorCode::InvalidCredentials, "ключ не принят (HTTP 401)"),
        403 => (ErrorCode::InsufficientPermissions, "нет доступа (HTTP 403)"),
        429 => (ErrorCode::RateLimited, "превышен лимит запросов (HTTP 429)"),
        500..=599 => (busy(op), "ошибка на стороне HTX (HTTP 5XX)"),
        400 => (rejected(op), "запрос не принят (HTTP 400)"),
        _ => (ErrorCode::BadResponse, "непонятный ответ"),
    };
    GatewayError::new(code, Some(venue()), format!("HTX: {text}, HTTP {status}")).with_raw(raw)
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
    GatewayError::new(code, Some(venue()), format!("HTX: {}", f.text(lim)))
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
        format!("HTX: непонятный ответ: {what}"),
    )
    .with_raw(raw)
}

/// «Запись не найдена» у HTX (ордер ещё или уже не виден) — по кодам документации. Ошибок нет.
pub fn is_not_found(e: &GatewayError) -> bool {
    ["base-record-invalid", "not-found", "base-not-found"]
        .iter()
        .any(|c| e.message.contains(&format!("код {c}:")))
}

/// Площадка HTX для ошибок.
fn venue() -> crate::core::types::VenueId {
    crate::core::types::VenueId(ID.to_string())
}

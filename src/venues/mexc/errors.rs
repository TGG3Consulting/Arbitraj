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
        // Ключ, подпись (Error Code: 10072, 700001, 700002, 602).
        10072 | 700001 | 700002 | 602 => ErrorCode::InvalidCredentials,
        // Время запроса (700003 «Timestamp for this request is outside of the recvWindow», 10073 «invalid
        // Request-Time»): ключ ни при чём — свой текст про часы (решение 118, А-🟠9). Запрос отклонён: у ордера и
        // вывода — отказ (не «итог неизвестен»), у остального — BadResponse.
        700003 | 10073 => return clock(code, msg, op, raw),
        // Права и блок аккаунта (700007, 401, 30020, 70011; 30035 «You have triggered the order limit, please contact
        // us by submitting a ticket», 60005 «your account is abnormal» — решение 118, А-🟡2).
        700007 | 401 | 30020 | 70011 | 30035 | 60005 => ErrorCode::InsufficientPermissions,
        700006 => ErrorCode::IpNotAllowed,
        // 429 «Too Many Requests» в теле — лимит, как HTTP 429 (решение 118, А-🔴3).
        429 => ErrorCode::RateLimited,
        // Баланс (10101 «does not have enough available balance», 30004 «Insufficient position», 30005 «Oversold»).
        10101 | 30004 | 30005 => ErrorCode::InsufficientBalance,
        // Решение 166: 16021 «Some pre-crediting assets are temporarily unavailable for withdrawal» — средства
        // временно заблокированы для вывода (предварительно зачисленный депозит, deposit_hold.rs). В таблице Error Code
        // кода нет — по живому ответу MEXC на вывод 04.11.2026 (сделка 20261104173851521BABYDOGE).
        16021 if op == Op::Withdraw => ErrorCode::WithdrawLocked,
        // Решение 253, дополнение t82 (Тигран 10.11), Error Code (https://www.mexc.com/api-docs/spot-v3/error-code,
        // сверено 11.11.2026): 10212 «This withdrawal address is not on the commonly used address list or has been
        // invalidated» — адрес (InvalidAddress); 157005 «Withdrawal is currently locked by withdrawal protection» (по
        // описанию — защита после смены пароля, 2FA или адресов), 10265 «Due to risk control, withdrawal is unavailable»
        // (описание: проверить действия аккаунта), 10098 «risk control system detected abnormal», 730100 «Unusual user
        // status» — аккаунт (AccountRestricted): монета не закрывается, алерт «проблема в аккаунте».
        10212 if op == Op::Withdraw => ErrorCode::InvalidAddress,
        157005 | 10265 | 10098 | 730100 if op == Op::Withdraw => ErrorCode::AccountRestricted,
        // 11444 «This feature is temporarily unavailable…» — однозначный отказ (решение 118, А-🟡3): у ордера и вывода
        // — отказ, а не «итог неизвестен».
        11444 if op == Op::Trade => ErrorCode::OrderRejected,
        11444 if op == Op::Withdraw => ErrorCode::WithdrawRejected,
        // Сервер занят: для ордера и вывода итог неизвестен (Timeout — нужна проверка), для остального — недоступно.
        500 | 503 | 504 | 20002 | 11444 if op != Op::Other => ErrorCode::Timeout,
        500 | 503 | 504 | 20002 | 11444 => ErrorCode::Unreachable,
        // Ордер не найден (-2011 «Unknown order sent»).
        -2011 => ErrorCode::BadResponse,
        _ => rejected(op),
    };
    let msg = if msg.is_empty() { "(без msg)" } else { msg };
    let text = format!("MEXC, код {code}: {msg}");
    GatewayError::new(unified, Some(venue()), text).with_raw(raw)
}

/// Код отказа по операции: ордер — OrderRejected, вывод — WithdrawRejected, остальное — BadResponse. Ошибок нет.
fn rejected(op: Op) -> ErrorCode {
    match op {
        Op::Trade => ErrorCode::OrderRejected,
        Op::Withdraw => ErrorCode::WithdrawRejected,
        Op::Move | Op::Other => ErrorCode::BadResponse,
    }
}

/// Время запроса вне recvWindow (700003, 10073): правило из введения документации — «if (timestamp < (serverTime +
/// 1000) && (serverTime - timestamp) <= recvWindow)». Принимает: код, msg, операцию, сырой ответ. Ошибок нет.
fn clock(code: i64, msg: &str, op: Op, raw: &str) -> GatewayError {
    let text = format!(
        "MEXC, код {code}: время запроса вне recvWindow — проверьте часы сервера (синхронизация времени) ({msg})"
    );
    GatewayError::new(rejected(op), Some(venue()), text).with_raw(raw)
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

/// Ошибка по HTTP-статусу. 403 — WAF; 418/429 — лимит (дальше — бан IP от 2 минут до 3 суток: «Continuing to exceed
/// limits after receiving 429 will result in an IP ban»), с Retry-After в тексте, если он пришёл; 5XX — сторона MEXC.
/// Принимает: статус, сырой ответ, Retry-After в секундах. Возвращает: GatewayError.
pub fn from_http(status: u16, raw: &str, retry_after_s: Option<u64>) -> GatewayError {
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
    let wait = retry_after_s
        .map(|s| format!(", Retry-After {s} с — запросы к MEXC не отправляются до его истечения"))
        .unwrap_or_default();
    GatewayError::new(
        code,
        Some(venue()),
        format!("MEXC: {text}, HTTP {status}{wait}"),
    )
    .with_raw(raw)
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

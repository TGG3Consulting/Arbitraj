//! Файл: venues/coinex/errors.rs
//! Что делает: перевод ошибок CoinEx (HTTP-статус и поле code ответа) в единые коды шлюза.
//! Кто вызывает: reply.rs, rest.rs и файлы адаптера.
//! От чего зависит: gateway/errors.rs, gateway/http.rs.
//! Настройки: нет.
//!
//! Официально (Error Handling, https://docs.coinex.com/api/v2/error): сначала HTTP-статус — не 200 «may come from
//! CoinEx's server gateway», тело не разбирать как JSON, записать сырым. Коды тела: 3008 «Service busy», 3109
//! «Insufficient balance», 3127 «Order size below minimum», 3606…3639 — цена/глубина/аукцион (отказ ордера), 3621 «cannot
//! fully execute; it was canceled» (FOK), 4001 «Service unavailable», 4002 «Request timed out», 4003 «Internal error»,
//! 4004 «Parameter error», 4005 «Invalid access_id», 4006 «Signature verification failed», 4007 «IP address blocked»,
//! 4008 «Invalid X-COIN-SIGN», 4009 «Invalid request method», 4010 «Expired request», 4011 «User barred from access»,
//! 4017 «Signature expired», 4018 «Endpoint deprecated», 4115 «User barred from trading», 4117 / 4158 «Trading barred»,
//! 4159 «API trading unavailable in the current market», 4213 «Rate limit triggered», 4512 «Insufficient sub-account
//! permissions». Rate Limit (https://docs.coinex.com/api/v2/rate-limit): после лимита — 3008, 4001, 4213. Кода «ордер не
//! найден» в документации нет; текст причины — поле message ответа как есть.

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

/// Коды «слишком часто / сервис занят» после лимита (страница Rate Limit) — пауза запросов (reply.rs).
pub const LIMIT_CODES: [&str; 3] = ["4213", "3008", "4001"];

/// Ошибка по полю code ответа. Принимает: код (строкой), message, операцию, сырой ответ. Возвращает: GatewayError.
pub fn from_code(code: &str, msg: &str, op: Op, raw: &str) -> GatewayError {
    let unified = match code {
        c if LIMIT_CODES.contains(&c) => ErrorCode::RateLimited,
        "4005" | "4006" | "4008" | "4010" | "4017" => ErrorCode::InvalidCredentials,
        "4007" => ErrorCode::IpNotAllowed,
        "4011" | "4115" | "4117" | "4158" | "4159" | "4512" => ErrorCode::InsufficientPermissions,
        "3109" => ErrorCode::InsufficientBalance,
        // «Request timed out» / «Internal error»: у денег итог неизвестен.
        "4002" | "4003" if op != Op::Other => ErrorCode::Timeout,
        "4002" | "4003" => ErrorCode::Unreachable,
        // Решение 268 (Тигран 14.11), Error Handling (https://docs.coinex.com/api/v2/error): 3606, 3615, 3618, 3616, 3632,
        // 3617, 3633 — отклонение цены ордера от последней / индексной цены и её границ: цена лимитного ордера вне
        // диапазона биржи (PriceOutOfRange). Только у операции ордера; рыночная продажа код снимает (market_sell.rs).
        // Дополнение 268 (ревью 2): только у операции ордера (Op::Trade); у прочих операций — как до решения 268.
        "3606" | "3615" | "3616" | "3617" | "3618" | "3632" | "3633" if op == Op::Trade => {
            ErrorCode::PriceOutOfRange
        }
        _ => rejected(op),
    };
    let msg = if msg.is_empty() {
        "(без message)"
    } else {
        msg
    };
    GatewayError::new(unified, Some(venue()), format!("CoinEx, код {code}: {msg}")).with_raw(raw)
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

/// Ответ не 200 — ответ шлюза CoinEx, тело не разбирается (Error Handling). Принимает: статус, операцию, сырой ответ.
/// Возвращает: GatewayError (5XX у денег — Timeout, дальше unsure делает «итог неизвестен»).
pub fn from_http(status: u16, op: Op, raw: &str) -> GatewayError {
    let code = match status {
        429 => ErrorCode::RateLimited,
        500..=599 => ErrorCode::Timeout,
        _ if op == Op::Other => ErrorCode::BadResponse,
        _ => ErrorCode::Timeout,
    };
    GatewayError::new(
        code,
        Some(venue()),
        format!("CoinEx: HTTP {status} — ответ шлюза CoinEx, не API (тело записано сырым)"),
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
    GatewayError::new(code, Some(venue()), format!("CoinEx: {}", f.text(lim)))
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
        format!("CoinEx: непонятный ответ: {what}"),
    )
    .with_raw(raw)
}

/// Площадка CoinEx для ошибок.
fn venue() -> crate::core::types::VenueId {
    crate::core::types::VenueId(ID.to_string())
}

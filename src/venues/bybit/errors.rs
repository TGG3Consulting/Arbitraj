//! Файл: venues/bybit/errors.rs
//! Что делает: перевод ошибок Bybit V5 (HTTP-статус и поле retCode ответа) в единые коды шлюза.
//! Кто вызывает: venues/bybit/reply.rs, rest.rs и файлы адаптера.
//! От чего зависит: gateway/errors.rs, gateway/http.rs (Fail).
//! Настройки: нет.
//!
//! Коды — по официальной таблице «Error Codes» https://bybit-exchange.github.io/docs/v5/error (разделы HTTP Code, UTA,
//! Spot Trade, Asset) и «Rate Limit Rules» https://bybit-exchange.github.io/docs/v5/rate-limit («403, access too
//! frequent» — бан IP, «retCode 10006 Too many visits!» — лимит UID). Текст причины — retMsg Bybit как есть.
//! Ответ на ордер «асинхронный» (Place Order: «The acknowledgement … indicates that the request was successfully
//! accepted») — серверные сбои у ордера, вывода и перевода дают «итог неизвестен», а не отказ.

use crate::gateway::errors::{ErrorCode, GatewayError};

use super::ctx::ID;

/// Для какой операции пришла ошибка: от этого зависит единый код у неизвестных кодов Bybit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    /// Ордера (постановка и отмена).
    Trade,
    /// Вывод.
    Withdraw,
    /// Перевод между своими счетами Bybit (FUND → UNIFIED): деньги двигаются — как вывод.
    Move,
    /// Всё остальное (справочники, балансы, проверки, запрос ордера).
    Other,
}

/// Ошибка по полю retCode. Принимает: код, retMsg, операцию, сырой ответ. Возвращает: GatewayError.
pub fn from_code(code: i64, msg: &str, op: Op, raw: &str) -> GatewayError {
    let unified = match code {
        // Ключ и подпись: 10003 «API key is invalid», 10004 «Error sign», 10007 «User authentication failed»,
        // -2015 «(Spot) Your api key has expired», 33004.
        10003 | 10004 | 10007 | -2015 | 33004 => ErrorCode::InvalidCredentials,
        // Время запроса: 10002 «The request time exceeds the time window range», -1 «request expired».
        10002 | -1 => return clock(code, msg, op, raw),
        // Права и блокировки: 10005 «Permission denied», 10008 «Common banned», 10009 «Service Restricted», 10024
        // «Compliance rules triggered», 10027 «Transactions are banned», 10028 «only … unified account users».
        // Решение 253, дополнение t85 (Тигран 11.11), Error Codes → Asset (https://bybit-exchange.github.io/docs/v5/error,
        // сверено 11.11.2026): у вывода 131004 «KYC needed», 131090 «User withdraw has been banned», 131091 «Blocked login
        // status does not allow withdrawals», 131092 «User status is abnormal» — аккаунт (AccountRestricted: алерт
        // «проблема в аккаунте», не ошибка подряд). У других операций — права, как раньше.
        131004 | 131090 | 131091 | 131092 if op == Op::Withdraw => ErrorCode::AccountRestricted,
        10005 | 10008 | 10009 | 10024 | 10027 | 10028 | 131004 | 131090 | 131091 | 131092 => {
            ErrorCode::InsufficientPermissions
        }
        10010 => ErrorCode::IpNotAllowed,
        // Лимиты: 10006 «Too many visits», 429 «high server load», 170005 «Too many new orders», 170222.
        10006 | 429 | 170005 | 170222 => ErrorCode::RateLimited,
        // Баланс: 110004, 110007, 110012 (UTA), 170131 «Balance insufficient», 131212 / 131228 (перевод),
        // 131085 «Withdrawal amount is greater than your availale balance».
        // Решение 166: 131085 у вывода — «(the deplayed withdrawal is triggered)»: задержка вывода, средства временно
        // не выводятся (withdrawable.rs, Get Withdrawable Amount) — WithdrawLocked.
        131085 if op == Op::Withdraw => ErrorCode::WithdrawLocked,
        110004 | 110007 | 110012 | 170131 | 131212 | 131228 | 131085 => {
            ErrorCode::InsufficientBalance
        }
        // Адрес вывода: 131083 «only allowed from address book», 131093 «not in the whitelist», 131066, 131098.
        131083 | 131093 | 131066 | 131098 if op == Op::Withdraw => ErrorCode::InvalidAddress,
        // Решение 253, дополнение t82 (Тигран 10.11), Error Codes → Asset (docs/v5/error.mdx репозитория
        // github.com/bybit-exchange/docs, сверено 11.11.2026): вывод отклонён из-за аккаунта — 131065 «Your KYC
        // information is incomplete», 131084 «Withdraw failed because of Uta Upgrading», 131086 «Withdrawal amount
        // exceeds risk limit (the risk limit of margin trade is triggered)», 131087 «your current account spot risk level
        // is too high, withdrawal is prohibited», 131088 «exceeds the remaining withdrawal limit of your identity
        // verification level», 131089 «User sensitive operation, withdrawal is prohibited within 24 hours», 131094
        // «UserId is not in the whitelist» — AccountRestricted (монета не закрывается, алерт «проблема в аккаунте»).
        131065 | 131084 | 131086 | 131087 | 131088 | 131089 | 131094 if op == Op::Withdraw => {
            ErrorCode::AccountRestricted
        }
        // Дополнение t85 (Тигран 11.11): 131095 «Withdrawl amount exceeds the 24 hour platform limit» — суточный лимит,
        // сбросится сам: WithdrawLimit (площадка на паузу до ручного снятия, core/withdraw_limit_pause.rs).
        131095 if op == Op::Withdraw => ErrorCode::WithdrawLimit,
        // Сервер: 10000 «Server Timeout», 10016 «Server error», 170001, 170007 «Timeout waiting for response from
        // backend server», 170146 «Order creation timeout», 170032, 170234, 131001, 131003, 131200, 131201, 131230.
        // Для ордера, вывода и перевода — итог неизвестен (Timeout → errors::unsure), для остального — недоступно.
        10000 | 10016 | 170001 | 170007 | 170146 | 170032 | 170234 | 131001 | 131003 | 131200
        | 131201 | 131230
            if op != Op::Other =>
        {
            ErrorCode::Timeout
        }
        10000 | 10016 | 170001 | 170007 | 170146 | 170032 | 170234 | 131001 | 131003 | 131200
        | 131201 | 131230 => ErrorCode::Unreachable,
        // Ордер не найден: 110001 «Order does not exist», 170213 «Order does not exist».
        110001 | 170213 => ErrorCode::BadResponse,
        _ => rejected(op),
    };
    let msg = if msg.is_empty() {
        "(без retMsg)"
    } else {
        msg
    };
    let text = format!("Bybit, код {code}: {msg}");
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

/// Время запроса вне окна (Integration Guidance: «server_time - recv_window <= timestamp < server_time + 1000»).
/// Запрос отклонён: у ордера и вывода — отказ (не «итог неизвестен»). Ошибок нет.
fn clock(code: i64, msg: &str, op: Op, raw: &str) -> GatewayError {
    let text = format!(
        "Bybit, код {code}: время запроса вне venue.recv_window_ms — проверьте часы сервера (синхронизация NTP) ({msg})"
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

/// Ошибка по HTTP-статусу (Error Codes → HTTP Code). 403 — «IP rate limit breached» / регион; 429 — «System level
/// frequency protection»; 401 — ключ; 5XX — сторона Bybit (итог неизвестен у денег). Ошибок нет.
pub fn from_http(status: u16, raw: &str) -> GatewayError {
    let (code, text) = match status {
        401 => (ErrorCode::InvalidCredentials, "неверный ключ (HTTP 401)"),
        403 => (
            ErrorCode::RateLimited,
            "HTTP 403: превышен лимит IP («access too frequent» — ждать не меньше 10 минут) или IP из запрещённого региона",
        ),
        429 => (
            ErrorCode::RateLimited,
            "HTTP 429: защита Bybit от частоты запросов",
        ),
        500..=599 => (
            ErrorCode::Timeout,
            "ошибка на стороне Bybit (HTTP 5XX) — итог операции неизвестен, нужна проверка",
        ),
        _ => (ErrorCode::BadResponse, "непонятный ответ"),
    };
    GatewayError::new(code, Some(venue()), format!("Bybit: {text}, HTTP {status}")).with_raw(raw)
}

/// Сбой HTTP-обмена: нет связи (Unreachable), запрос мог дойти без ответа (Timeout), тело не текст или больше
/// предела (BadResponse). Ошибок нет.
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
    GatewayError::new(code, Some(venue()), format!("Bybit: {}", f.text(lim)))
}

/// Сбой обмена у операции с деньгами: любой сбой, кроме «соединение не установлено», — «результат неизвестен».
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
        format!("Bybit: непонятный ответ: {what}"),
    )
    .with_raw(raw)
}

/// Площадка Bybit для ошибок.
fn venue() -> crate::core::types::VenueId {
    crate::core::types::VenueId(ID.to_string())
}

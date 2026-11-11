//! Файл: venues/xt/errors.rs
//! Что делает: перевод ошибок XT.com (HTTP-статус и поля rc/mc ответа) в единые коды шлюза.
//! Кто вызывает: reply.rs, rest.rs и файлы адаптера.
//! От чего зависит: gateway/errors.rs.
//! Настройки: нет.
//!
//! Коды — по официальной таблице https://doc.xt.com/#documentationerrorCode (`_documentation/errorCode.md`): ответ
//! {"rc": 0|1, "mc": "<код>", "ma": [аргументы], "result"}; rc 1 — отказ, причина — mc. AUTH_001…007 — нет или неверен
//! заголовок подписи, AUTH_101 ключа нет, AUTH_102 ключ не активирован, AUTH_103 подпись, AUTH_104 «Unbound IP
//! request», AUTH_105 «outdated message» (часы/recvWindow — FAQ.md), AUTH_106 «Exceeded apikey permission»;
//! ORDER_002, WITHDRAW_011, FUND_002, TRANSFER_002 — не хватает средств; ORDER_005 — ордера нет; COMMON_002 «System
//! busy», COMMON_003 «Operation failed, please try it later» — выполнено ли, не сказано: у ордера и вывода — «итог
//! неизвестен»; GATEWAY_0001…0004 «Trigger risk control» — без описания: как лимит (пауза, ничего не слать).
//! HTTP: 429 — «too frequent… the IP or apiKey will be blocked» (limitRules.md), 404 — нет интерфейса, 500/502/503 —
//! сбой сервиса (итог операции неизвестен). Текст ошибки — «XT, код <mc>: <ma>» (решение 164: notify/reason.rs).

use crate::gateway::errors::{ErrorCode, GatewayError};

use super::ctx::ID;

/// Для какой операции пришла ошибка: от этого зависит единый код у неизвестных кодов.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    /// Ордера.
    Trade,
    /// Вывод.
    Withdraw,
    /// Всё остальное (справочники, балансы, проверки, запрос и отмена ордера).
    Other,
}

/// Код mc «риск-контроль шлюза» — ведёт себя как лимит (ничего не слать до паузы). Ошибок нет.
pub fn is_risk_control(mc: &str) -> bool {
    mc.starts_with("GATEWAY_")
}

/// Ошибка по полю mc ответа. Принимает: код, аргументы ma (текстом), операцию, сырой ответ. Возвращает: GatewayError.
pub fn from_code(mc: &str, ma: &str, op: Op, raw: &str) -> GatewayError {
    let unified = match mc {
        "AUTH_001" | "AUTH_002" | "AUTH_003" | "AUTH_004" | "AUTH_005" | "AUTH_006"
        | "AUTH_007" | "AUTH_101" | "AUTH_102" | "AUTH_103" => ErrorCode::InvalidCredentials,
        "AUTH_104" => ErrorCode::IpNotAllowed,
        "AUTH_105" => return clock(mc, ma, op, raw),
        "AUTH_106" => ErrorCode::InsufficientPermissions,
        "ORDER_002" | "WITHDRAW_011" | "FUND_002" | "TRANSFER_002" => {
            ErrorCode::InsufficientBalance
        }
        m if is_risk_control(m) => ErrorCode::RateLimited,
        // Решение 253, дополнение t82 (Тигран 10.11), таблица кодов (https://doc.xt.com/#documentationerrorCode; сверено
        // 11.11.2026 по копии https://xt-com.github.io/xt4-api/): адрес — WITHDRAW_002 «The withdrawal address is
        // invalid», WITHDRAW_004 «The withdrawal address is not added», WITHDRAW_014 «This withdrawal address cannot be
        // used in the internal transfer function» (InvalidAddress); аккаунт — WITHDRAW_003 «account security level is
        // low, please bind any two security verifications» (AccountRestricted). Дополнение t85 (Тигран 11.11): дневной
        // лимит вывода аккаунта, сбросится сам — WITHDRAW_012 / WITHDRAW_013 «remaining withdrawal limit today is not
        // enough», WITHDRAW_020 / WITHDRAW_021 «withdrawal amount has reached the upper limit for today» (WithdrawLimit:
        // площадка на паузу до ручного снятия, core/withdraw_limit_pause.rs).
        "WITHDRAW_002" | "WITHDRAW_004" | "WITHDRAW_014" if op == Op::Withdraw => {
            ErrorCode::InvalidAddress
        }
        "WITHDRAW_003" if op == Op::Withdraw => ErrorCode::AccountRestricted,
        "WITHDRAW_012" | "WITHDRAW_013" | "WITHDRAW_020" | "WITHDRAW_021" if op == Op::Withdraw => {
            ErrorCode::WithdrawLimit
        }
        // Выполнено ли — не сказано: у ордера и вывода — проверка по своему id (unsure → OutcomeUnknown).
        "COMMON_002" | "COMMON_003" if op != Op::Other => ErrorCode::Timeout,
        "COMMON_002" | "COMMON_003" => ErrorCode::Unreachable,
        // Ордера нет — не отказ денег, вызывающий решает по смыслу.
        "ORDER_005" => ErrorCode::BadResponse,
        _ => rejected(op),
    };
    let ma = if ma.is_empty() || ma == "[]" {
        "(без пояснения)"
    } else {
        ma
    };
    GatewayError::new(unified, Some(venue()), format!("XT, код {mc}: {ma}")).with_raw(raw)
}

/// Код отказа по операции: ордер — OrderRejected, вывод — WithdrawRejected, остальное — BadResponse. Ошибок нет.
fn rejected(op: Op) -> ErrorCode {
    match op {
        Op::Trade => ErrorCode::OrderRejected,
        Op::Withdraw => ErrorCode::WithdrawRejected,
        Op::Other => ErrorCode::BadResponse,
    }
}

/// AUTH_105 «outdated message»: свой текст про часы сервера и recvWindow (FAQ.md). Запрос отклонён — у ордера и вывода
/// отказ, не «итог неизвестен». Ошибок нет.
fn clock(mc: &str, ma: &str, op: Op, raw: &str) -> GatewayError {
    let text = format!(
        "XT, код {mc}: запрос устарел — проверьте часы сервера и venue.recv_window_ms ({ma})"
    );
    GatewayError::new(rejected(op), Some(venue()), text).with_raw(raw)
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

/// Ошибка по HTTP-статусу (errorCode.md, limitRules.md). Принимает: статус, сырой ответ, пояснение паузы.
/// Возвращает: GatewayError.
pub fn from_http(status: u16, raw: &str, pause: &str) -> GatewayError {
    let (code, text) = match status {
        429 => (
            ErrorCode::RateLimited,
            "слишком частые запросы (HTTP 429) — XT блокирует IP или ключ при продолжении",
        ),
        404 => (ErrorCode::BadResponse, "интерфейс не существует (HTTP 404)"),
        500..=599 => (
            ErrorCode::Timeout,
            "сбой сервиса XT (HTTP 5XX) — итог операции неизвестен, нужна проверка",
        ),
        _ => (ErrorCode::BadResponse, "непонятный ответ"),
    };
    let pause = if pause.is_empty() {
        String::new()
    } else {
        format!("; {pause}")
    };
    GatewayError::new(
        code,
        Some(venue()),
        format!("XT: {text}, HTTP {status}{pause}"),
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
    GatewayError::new(code, Some(venue()), format!("XT: {}", f.text(lim)))
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
        format!("XT: непонятный ответ: {what}"),
    )
    .with_raw(raw)
}

/// Площадка XT для ошибок. Ошибок нет.
fn venue() -> crate::core::types::VenueId {
    crate::core::types::VenueId(ID.to_string())
}

//! Файл: venues/kraken/errors.rs
//! Что делает: перевод ошибок Kraken в единые коды шлюза: у спот REST — строки массива error[] («EGeneral:Invalid
//! arguments»), у Funding (Beta) — HTTP-статус не 2xx с телом ошибки; сбои обмена — «нет связи» / «итог неизвестен».
//! Кто вызывает: reply.rs, rest.rs и файлы адаптера.
//! От чего зависит: gateway/errors.rs, gateway/http.rs.
//! Настройки: нет.
//!
//! Официально: ошибки — массив строк вида «<категория>:<текст>[:<подробность>]» (OpenAPI https://docs.kraken.com/openapi/
//! spot-rest.yaml, схема error: «Kraken API error», пример «EGeneral:Invalid arguments»); HTTP 429 и Retry-After в
//! документации лимитов нет — «EAPI:Rate limit exceeded», «EOrder:Rate limit exceeded», «EOrder:Orders limit exceeded»,
//! «EService: Throttled: [UNIX timestamp]» ([RL-REST], [RL-TRADE] — docs/notes/kraken-analysis-2026-11-05.md);
//! «EAPI:Invalid nonce» ([AUTH]); «EFunding:Max fee exceeded» (Withdraw). Строки ключа, подписи, прав и баланса
//! (EAPI:Invalid key, EAPI:Invalid signature, EGeneral:Permission denied, EOrder:Insufficient funds, EService:
//! Unavailable / Busy) — по таблице ошибок Kraken, которую из среды разработки прочитать не удалось: сверить на живых
//! ответах. Неизвестная строка — отказ операции с текстом Kraken как есть (не угадываем смысл).
//! Решение 264 (гид https://docs.kraken.com/api/docs/guides/spot-errors): «EOrder:Domain rate limit exceeded» — лимит
//! (пауза); у ордера «EService:Deadline elapsed» («the order may have been placed despite the timeout») и
//! «EGeneral:Internal error» (офдока о судьбе ордера молчит) — «итог неизвестен», не отказ.
//! Текст ошибки — «Kraken, ошибка <строка Kraken>» (так его разбирает notify/reason.rs, решение 164).

use crate::gateway::errors::{ErrorCode, GatewayError};

use super::ctx::ID;

/// Приставка текста ошибки Kraken — по ней notify/reason.rs находит код (решение 164).
pub const PREFIX: &str = "Kraken, ошибка ";

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

/// Строка «слишком часто» (счётчик ключа или торгового движка, одновременные запросы). Ошибок нет.
pub fn is_rate_limit(e: &str) -> bool {
    let e = e.replace(' ', "");
    e.starts_with("EAPI:Ratelimitexceeded")
        || e.starts_with("EOrder:Ratelimitexceeded")
        || e.starts_with("EOrder:Orderslimitexceeded")
        // Решение 264, офдока guides/spot-errors: «The domain rate limit (master + all sub-accounts) has been reached».
        || e.starts_with("EOrder:Domainratelimitexceeded")
        || e.starts_with("EService:Throttled")
}

/// Единый код по строке ошибки Kraken. Ошибок нет.
fn unified(e: &str, op: Op) -> ErrorCode {
    let s = e.replace(' ', "");
    let has = |p: &str| s.starts_with(p);
    if has("EAPI:Invalidkey") || has("EAPI:Invalidsignature") {
        ErrorCode::InvalidCredentials
    } else if has("EAPI:Invalidnonce") {
        // Nonce не принят — запрос не исполнен: отказ операции (не «итог неизвестен»).
        rejected(op)
    } else if has("EGeneral:Permissiondenied") {
        ErrorCode::InsufficientPermissions
    } else if is_rate_limit(e) {
        ErrorCode::RateLimited
    } else if has("EOrder:Insufficientfunds") {
        ErrorCode::InsufficientBalance
    } else if op == Op::Trade && (has("EService:Deadlineelapsed") || has("EGeneral:Internalerror"))
    {
        // Решение 264 (https://docs.kraken.com/api/docs/guides/spot-errors): Deadline elapsed — «check OpenOrders —
        // the order may have been placed despite the timeout»; Internal error — «An unexpected error occurred on
        // Kraken's side», встал ли ордер, офдока не говорит. Оба у ордера — Timeout → unsure → «итог неизвестен»
        // (поиск по cl_ord_id найдёт ордер или нет), не отказ.
        ErrorCode::Timeout
    } else if has("EService:Unavailable") || has("EService:Busy") {
        if op == Op::Other {
            ErrorCode::Unreachable
        } else {
            ErrorCode::Timeout
        }
    } else {
        rejected(op)
    }
}

/// Ошибка по строкам error[] спот REST. Принимает: строки, операцию, сырой ответ. Возвращает: GatewayError.
pub fn from_errors(list: &[String], op: Op, raw: &str) -> GatewayError {
    let first = list.first().map(String::as_str).unwrap_or("(пустой error)");
    let code = unified(first, op);
    GatewayError::new(code, Some(venue()), format!("{PREFIX}{}", list.join("; "))).with_raw(raw)
}

/// Отказ по операции: ордер — OrderRejected, вывод — WithdrawRejected, остальное — BadResponse. Ошибок нет.
pub fn rejected(op: Op) -> ErrorCode {
    match op {
        Op::Trade => ErrorCode::OrderRejected,
        Op::Withdraw => ErrorCode::WithdrawRejected,
        Op::Other => ErrorCode::BadResponse,
    }
}

/// Ответ Funding (Beta) не 2xx: «Non-2xx Kraken API response with a typed error body» (гид Funding) — вид тела в
/// документации не описан, текст — как есть (обрезан). 401 — ключ; 403 — права; 429 — лимит; 5XX — у денег итог
/// неизвестен. Принимает: статус, тело, операцию. Возвращает: GatewayError.
pub fn from_funding(status: u16, raw: &str, op: Op) -> GatewayError {
    let code = match status {
        401 => ErrorCode::InvalidCredentials,
        403 => ErrorCode::InsufficientPermissions,
        429 => ErrorCode::RateLimited,
        500..=599 if op == Op::Other => ErrorCode::Unreachable,
        500..=599 => ErrorCode::Timeout,
        _ => rejected(op),
    };
    let body: String = raw.chars().take(300).collect();
    GatewayError::new(
        code,
        Some(venue()),
        format!("Kraken Funding, HTTP {status}: {body}"),
    )
    .with_raw(raw)
}

/// Ответ спот REST без понятного тела. 5XX — сторона Kraken. Принимает: статус, сырой ответ. Возвращает: GatewayError.
pub fn from_http(status: u16, raw: &str) -> GatewayError {
    let (code, text) = match status {
        429 => (ErrorCode::RateLimited, "превышен лимит запросов (HTTP 429)"),
        500..=599 => (
            ErrorCode::Timeout,
            "ошибка на стороне Kraken (HTTP 5XX) — итог операции неизвестен",
        ),
        _ => (ErrorCode::BadResponse, "непонятный ответ"),
    };
    GatewayError::new(
        code,
        Some(venue()),
        format!("Kraken: {text}, HTTP {status}"),
    )
    .with_raw(raw)
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
    GatewayError::new(code, Some(venue()), format!("Kraken: {}", f.text(lim)))
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
        format!("Kraken: непонятный ответ: {what}"),
    )
    .with_raw(raw)
}

/// Площадка Kraken для ошибок.
fn venue() -> crate::core::types::VenueId {
    crate::core::types::VenueId(ID.to_string())
}

//! Файл: venues/okx/errors.rs
//! Что делает: перевод ошибок OKX (HTTP-статус, поле code ответа или sCode строки ответа) в единые коды шлюза.
//! Кто вызывает: reply.rs, rest.rs и файлы адаптера.
//! От чего зависит: gateway/errors.rs, gateway/http.rs.
//! Настройки: нет.
//!
//! Коды — по официальной таблице https://www.okx.com/docs-v5/en/ «Error Code → REST API» (код, HTTP-статус, текст):
//! 50011 «Too Many Requests» и 50013 «Systems are busy» (HTTP 429), 50040 «Too frequent operations», 50061 «maximum order
//! rate limit for this account», 58102 «Rate limit reached» (HTTP 429); 50101–50114 — ключ, время, подпись (HTTP 401),
//! 50102 «Timestamp request expired»; 50119 «API key doesn't exist», 50120 «doesn't have permission», 50121 «can't access
//! our services through the IP address», 50100 «API frozen»; 50001 (503), 50026 (500) — сторона OKX; 51008 «Insufficient
//! … balance», 58350 «Insufficient balance»; 51001 «Instrument ID … doesn't exist», 51020 «minimum order amount», 51121
//! «multiple of the lot size», 51137/51138 пределы цены, 51155 «restricted due to local compliance»; 58200–58210, 58237 —
//! отказ вывода (58207 «isn't on the verified address list», 58237 «provide accurate recipient information (rcvrInfo)»);
//! 58004 «Account blocked», 58100/58101 перевод приостановлен. Текст причины — msg / sMsg ответа OKX как есть.

use crate::gateway::errors::{ErrorCode, GatewayError};

use super::ctx::ID;

/// Для какой операции пришла ошибка: от неё зависит единый код у отказов и «итог неизвестен».
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    /// Ордера.
    Trade,
    /// Вывод.
    Withdraw,
    /// Перевод между своими счетами OKX (Funding ↔ Trading): деньги двигаются — как вывод.
    Move,
    /// Всё остальное (справочники, балансы, проверки, запрос и отмена ордера).
    Other,
}

/// Коды «слишком часто» — после них площадка ставит паузу (reply.rs). Ошибок нет.
pub fn is_rate_limit(code: &str) -> bool {
    matches!(code, "50011" | "50013" | "50040" | "50061" | "58102")
}

/// Ошибка по коду OKX. Принимает: код, текст, операцию, сырой ответ. Возвращает: GatewayError.
pub fn from_code(code: &str, msg: &str, op: Op, raw: &str) -> GatewayError {
    let unified = match code {
        "50101" | "50103" | "50104" | "50105" | "50111" | "50113" | "50114" | "50119" => {
            ErrorCode::InvalidCredentials
        }
        // 50102 «Timestamp request expired», 50112 «Invalid OK-ACCESS-TIMESTAMP» — часы сервера.
        "50102" | "50112" => return clock(code, msg, op, raw),
        "50121" => ErrorCode::IpNotAllowed,
        "50100" | "50120" | "58004" | "51155" | "58100" | "58101" | "58104" => {
            ErrorCode::InsufficientPermissions
        }
        c if is_rate_limit(c) => ErrorCode::RateLimited,
        // 51008 (и подкоды 51008_*) — «Insufficient {param0} balance», 58350 «Insufficient balance».
        c if c == "58350" || c.starts_with("51008") => ErrorCode::InsufficientBalance,
        "58207" | "58203" | "58213" => ErrorCode::InvalidAddress,
        "58200" | "58201" | "58202" | "58204" | "58205" | "58206" | "58208" | "58209" | "58210"
        | "58237" => ErrorCode::WithdrawRejected,
        // Сторона OKX (50001 503, 50026 500, 50004 «API endpoint request timeout»): у денег итог неизвестен.
        "50001" | "50004" | "50026" if op != Op::Other => ErrorCode::Timeout,
        "50001" | "50004" | "50026" => ErrorCode::Unreachable,
        _ => rejected(op),
    };
    let msg = if msg.is_empty() { "(без msg)" } else { msg };
    GatewayError::new(unified, Some(venue()), format!("OKX, код {code}: {msg}")).with_raw(raw)
}

/// Отказ по операции: ордер — OrderRejected, вывод — WithdrawRejected, остальное — BadResponse. Ошибок нет.
pub fn rejected(op: Op) -> ErrorCode {
    match op {
        Op::Trade => ErrorCode::OrderRejected,
        Op::Withdraw => ErrorCode::WithdrawRejected,
        Op::Move | Op::Other => ErrorCode::BadResponse,
    }
}

/// 50102 / 50112: время запроса не принято. Ошибок нет.
fn clock(code: &str, msg: &str, op: Op, raw: &str) -> GatewayError {
    let text = format!(
        "OKX, код {code}: OK-ACCESS-TIMESTAMP не принят — проверьте часы сервера (расхождение не больше 30 с) ({msg})"
    );
    GatewayError::new(rejected(op), Some(venue()), text).with_raw(raw)
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

/// Ошибка по HTTP-статусу (тело без понятного code). 401 — ключ; 403 — «API key to be bound to an IP» (50035) и
/// прочие запреты; 429 — лимит; 5XX — сторона OKX. Принимает: статус, сырой ответ. Возвращает: GatewayError.
pub fn from_http(status: u16, raw: &str) -> GatewayError {
    let (code, text) = match status {
        401 => (ErrorCode::InvalidCredentials, "ключ не принят (HTTP 401)"),
        403 => (
            ErrorCode::InsufficientPermissions,
            "запрос запрещён (HTTP 403)",
        ),
        429 => (
            ErrorCode::RateLimited,
            "превышен лимит запросов (HTTP 429) — пауза venue.rate_limit_pause_ms",
        ),
        500..=599 => (
            ErrorCode::Timeout,
            "ошибка на стороне OKX (HTTP 5XX) — итог операции неизвестен",
        ),
        _ => (ErrorCode::BadResponse, "непонятный ответ"),
    };
    GatewayError::new(code, Some(venue()), format!("OKX: {text}, HTTP {status}")).with_raw(raw)
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
    GatewayError::new(code, Some(venue()), format!("OKX: {}", f.text(lim)))
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
        format!("OKX: непонятный ответ: {what}"),
    )
    .with_raw(raw)
}

/// Площадка OKX для ошибок.
fn venue() -> crate::core::types::VenueId {
    crate::core::types::VenueId(ID.to_string())
}

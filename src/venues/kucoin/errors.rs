//! Файл: venues/kucoin/errors.rs
//! Что делает: перевод ошибок KuCoin (HTTP-статус и поле code ответа) в единые коды шлюза.
//! Кто вызывает: reply.rs, rest.rs и файлы адаптера.
//! От чего зависит: gateway/errors.rs, gateway/http.rs.
//! Настройки: нет.
//!
//! Коды — по официальным таблицам «Classic Spot» https://www.kucoin.com/docs-new/error-code/spot и «Classic HTTP»
//! https://www.kucoin.com/docs-new/error-code/http; 429000 — https://www.kucoin.com/docs-new/rate-limit-rule-classic.
//! Текст причины — поле msg ответа KuCoin как есть.

use crate::gateway::errors::{ErrorCode, GatewayError};

use super::ctx::ID;

/// Для какой операции пришла ошибка: от неё зависит единый код у отказов и «итог неизвестен».
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    /// Ордера.
    Trade,
    /// Вывод.
    Withdraw,
    /// Перевод между своими счетами KuCoin (main ↔ trade): деньги двигаются — как вывод.
    Move,
    /// Всё остальное (справочники, балансы, проверки, запрос и отмена ордера).
    Other,
}

/// Ошибка по полю code ответа. Принимает: код (строка), msg, операцию, сырой ответ. Возвращает: GatewayError.
pub fn from_code(code: &str, msg: &str, op: Op, raw: &str) -> GatewayError {
    let unified = match code {
        // 400001 нет заголовка KC-API-*, 400003 ключа нет, 400004 неверная passphrase, 400005 подпись.
        "400001" | "400003" | "400004" | "400005" => ErrorCode::InvalidCredentials,
        // 400002 «Invalid KC-API-TIMESTAMP» — часы сервера, запрос отклонён.
        "400002" => return clock(msg, op, raw),
        "400006" => ErrorCode::IpNotAllowed,
        // 400007 доступ запрещён, 411100 аккаунт заморожен, 400700 ограничение по риску, 400500 недоступно в регионе,
        // 200009 нужна KYC.
        "400007" | "411100" | "400700" | "400500" | "200009" => ErrorCode::InsufficientPermissions,
        "429000" => ErrorCode::RateLimited,
        // Баланс: 200004, 102421; 115007 — «withdrawals only come from the funds account» (вывод только с main).
        "200004" | "102421" | "115007" => ErrorCode::InsufficientBalance,
        // Решение 253, дополнение t82 (Тигран 10.11): 110003 «Withdrawal limit exceeded» — по таблице Classic Spot
        // (https://www.kucoin.com/docs-new/error-code/spot, сверено 11.11.2026) суточный предел выводов аккаунта (24 ч,
        // сумма) — аккаунт (AccountRestricted), не монета.
        "110003" if op == Op::Withdraw => ErrorCode::AccountRestricted,
        // 260210 «withdraw.disabled -- Currency/Chain withdraw is closed, or user is frozen to withdraw» — по одному коду
        // монета ИЛИ аккаунт: различить нельзя — как раньше (отказ вывода, закрытие входа по монете).
        "260210" | "110003" => ErrorCode::WithdrawRejected,
        // Решение 166: 115008 «Insufficient withdrawable assets: Assets equivalent to … USDT (… ION) are currently locked
        // from withdrawal» — средства временно заблокированы для вывода (квота вывода, withdraw_quota.rs). Смысл — по
        // живому ответу KuCoin на вывод 05.11.2026 (сделка 20261105020010981ION); с таблицей Classic Spot сверить.
        "115008" if op == Op::Withdraw => ErrorCode::WithdrawLocked,
        // Сервер (500000 «Internal server error», 230005 «System busy»): у денег итог неизвестен.
        "500000" | "230005" if op != Op::Other => ErrorCode::Timeout,
        "500000" | "230005" => ErrorCode::Unreachable,
        _ => rejected(op),
    };
    let msg = if msg.is_empty() { "(без msg)" } else { msg };
    GatewayError::new(unified, Some(venue()), format!("KuCoin, код {code}: {msg}")).with_raw(raw)
}

/// Отказ по операции: ордер — OrderRejected, вывод — WithdrawRejected, остальное — BadResponse. Ошибок нет.
pub fn rejected(op: Op) -> ErrorCode {
    match op {
        Op::Trade => ErrorCode::OrderRejected,
        Op::Withdraw => ErrorCode::WithdrawRejected,
        Op::Move | Op::Other => ErrorCode::BadResponse,
    }
}

/// 400002: время запроса не принято. Ошибок нет.
fn clock(msg: &str, op: Op, raw: &str) -> GatewayError {
    let text = format!(
        "KuCoin, код 400002: KC-API-TIMESTAMP не принят — проверьте часы сервера (синхронизация времени) ({msg})"
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

/// Ошибка по HTTP-статусу (тело без понятного code). 401 — ключ; 403 — «forbidden, or a rate limit was exceeded»;
/// 429 — лимит; 5XX — сторона KuCoin. Принимает: статус, сырой ответ. Возвращает: GatewayError.
pub fn from_http(status: u16, raw: &str) -> GatewayError {
    let (code, text) = match status {
        401 => (ErrorCode::InvalidCredentials, "ключ не принят (HTTP 401)"),
        403 => (
            ErrorCode::InsufficientPermissions,
            "запрос запрещён или превышен лимит (HTTP 403)",
        ),
        429 => (
            ErrorCode::RateLimited,
            "превышен лимит запросов (HTTP 429) — пауза до сброса окна",
        ),
        500..=599 => (
            ErrorCode::Timeout,
            "ошибка на стороне KuCoin (HTTP 5XX) — итог операции неизвестен",
        ),
        _ => (ErrorCode::BadResponse, "непонятный ответ"),
    };
    GatewayError::new(
        code,
        Some(venue()),
        format!("KuCoin: {text}, HTTP {status}"),
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
    GatewayError::new(code, Some(venue()), format!("KuCoin: {}", f.text(lim)))
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
        format!("KuCoin: непонятный ответ: {what}"),
    )
    .with_raw(raw)
}

/// Площадка KuCoin для ошибок.
fn venue() -> crate::core::types::VenueId {
    crate::core::types::VenueId(ID.to_string())
}

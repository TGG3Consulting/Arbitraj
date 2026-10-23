//! Файл: venues/bingx/errors.rs
//! Что делает: перевод ошибок BingX (HTTP-статус и поле code ответа) в единые коды шлюза.
//! Кто вызывает: venues/bingx/rest.rs.
//! От чего зависит: gateway/errors.rs.
//! Настройки: нет.
//!
//! Коды — по официальной документации: «Error Code Reference»
//! https://bingx-api.github.io/docs-v3/#/en/Quick%20Start/Error%20Code%20Reference
//! и https://github.com/BingX-API/api-ai-skills/blob/main/skills/references/error-codes.md
//! Текст причины всегда берётся из поля msg ответа BingX — он виден Тиграну как есть.

use crate::gateway::errors::{ErrorCode, GatewayError};

use super::ctx::ID;

/// Для какой операции пришла ошибка: от этого зависит единый код у неизвестных кодов BingX.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    /// Ордера.
    Trade,
    /// Вывод.
    Withdraw,
    /// Всё остальное (справочники, балансы, проверки).
    Other,
}

/// Ошибка по полю code ответа BingX.
/// Принимает: код, текст msg, операцию, сырой ответ. Возвращает: GatewayError.
pub fn from_code(code: i64, msg: &str, op: Op, raw: &str) -> GatewayError {
    let unified = match code {
        100001 | 100412 | 100413 => ErrorCode::InvalidCredentials,
        100004 => ErrorCode::InsufficientPermissions,
        100419 => ErrorCode::IpNotAllowed,
        100410 => ErrorCode::RateLimited,
        100202 => ErrorCode::InsufficientBalance,
        // «Система занята / запрос не выполнен»: для ордера и вывода итог неизвестен — Timeout
        // (нужна проверка), для остального — площадка недоступна.
        100500 if op != Op::Other => ErrorCode::Timeout,
        100500 => ErrorCode::Unreachable,
        _ => match op {
            Op::Trade => ErrorCode::OrderRejected,
            Op::Withdraw => ErrorCode::WithdrawRejected,
            Op::Other => ErrorCode::BadResponse,
        },
    };
    let text = format!("BingX, код {code}: {msg}");
    GatewayError::new(unified, Some(venue()), text).with_raw(raw)
}

/// Ошибка по HTTP-статусу (когда тело не в формате BingX).
/// 418 — IP заблокирован после превышения лимита; 429 — слишком частые запросы.
pub fn from_http(status: u16, raw: &str) -> GatewayError {
    let (code, text) = match status {
        401 => (
            ErrorCode::InvalidCredentials,
            "неверный API-ключ (HTTP 401)",
        ),
        403 => (ErrorCode::InsufficientPermissions, "нет доступа (HTTP 403)"),
        418 => (
            ErrorCode::RateLimited,
            "IP заблокирован за превышение лимита запросов (HTTP 418) — прекратить запросы и ждать",
        ),
        429 => (ErrorCode::RateLimited, "слишком частые запросы (HTTP 429)"),
        504 => (
            ErrorCode::Timeout,
            "шлюз BingX не дождался ответа (HTTP 504) — результат нужно проверить",
        ),
        _ => (ErrorCode::BadResponse, "непонятный ответ"),
    };
    GatewayError::new(code, Some(venue()), format!("BingX: {text}, HTTP {status}")).with_raw(raw)
}

/// Сетевая ошибка (нет связи или таймаут). Адрес из ошибки удаляется.
pub fn from_network(e: reqwest::Error) -> GatewayError {
    let code = if e.is_timeout() {
        ErrorCode::Timeout
    } else {
        ErrorCode::Unreachable
    };
    GatewayError::new(
        code,
        Some(venue()),
        format!("BingX: нет связи: {}", e.without_url()),
    )
}

/// Ответ не удалось разобрать.
pub fn bad(what: &str, raw: &str) -> GatewayError {
    GatewayError::new(
        ErrorCode::BadResponse,
        Some(venue()),
        format!("BingX: непонятный ответ: {what}"),
    )
    .with_raw(raw)
}

/// Идентификатор площадки для ошибок.
fn venue() -> crate::core::types::VenueId {
    crate::core::types::VenueId(ID.to_string())
}

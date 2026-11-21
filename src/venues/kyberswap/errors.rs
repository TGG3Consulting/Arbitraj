//! Файл: venues/kyberswap/errors.rs
//! Что делает: перевод ответов API агрегатора KyberSwap и узла BSC (HTTP-статус, сбой сети, ошибка JSON-RPC узла) в
//! единые коды шлюза (gateway/errors.rs). Тексты причин берутся из ответа как есть — их видит Тигран.
//! Кто вызывает: rest.rs (ответы агрегатора), node.rs (ответы узла).
//! От чего зависит: gateway/errors.rs, gateway/http.rs, ctx.rs (ID).
//! Настройки: нет.
//!
//! Источники: формат ошибки JSON-RPC {code, message} — https://www.jsonrpc.org/specification#error_object ; тексты
//! узла BSC (nonce too low, insufficient funds, underpriced, execution reverted) —
//! https://github.com/bnb-chain/bsc/blob/947ba1cb1ef05d4a5f71b738113a3d8b4c16f6b7/core/error.go ;
//! формат ответа агрегатора {code, message, data} — официальная спецификация aggregator-api (evm-swaps).

use crate::core::types::VenueId;
use crate::gateway::errors::{ErrorCode, GatewayError};
use crate::gateway::http::{Fail, Limits};

use super::ctx::ID;

/// Идентификатор площадки для ошибок без окружения Ctx.
fn venue() -> VenueId {
    VenueId(ID.to_string())
}

/// Ошибка из поля error ответа JSON-RPC узла. Принимает: код, текст, источник, сырой ответ. Возвращает: GatewayError.
pub fn from_rpc(code: i64, message: &str, source: &str, raw: &str) -> GatewayError {
    let m = message.to_ascii_lowercase();
    let unified = if m.contains("nonce too low") {
        ErrorCode::NonceConflict
    } else if m.contains("insufficient funds") {
        ErrorCode::InsufficientBalance
    } else if m.contains("underpriced") {
        ErrorCode::GasBelowChannelMin
    } else if m.contains("execution reverted") {
        ErrorCode::SwapReverted
    } else {
        ErrorCode::BadResponse
    };
    GatewayError::new(
        unified,
        Some(venue()),
        format!("{source}, код {code}: {message}"),
    )
    .with_raw(raw)
}

/// Ошибка по HTTP-статусу (тело — не разобранный полезный ответ).
/// Принимает: статус, источник, сырой ответ. Возвращает: GatewayError.
pub fn from_http(status: u16, source: &str, raw: &str) -> GatewayError {
    let (code, text) = match status {
        401 | 403 => (
            ErrorCode::InvalidCredentials,
            "доступ не разрешён — проверьте ключ/идентификатор",
        ),
        429 => (ErrorCode::RateLimited, "превышен лимит запросов"),
        404 => (ErrorCode::NoRoute, "маршрут или ресурс не найден"),
        500..=599 => (ErrorCode::Unreachable, "ошибка на стороне сервера"),
        _ => (ErrorCode::BadResponse, "непонятный ответ"),
    };
    GatewayError::new(
        code,
        Some(venue()),
        format!("{source}: {text}, HTTP {status}"),
    )
    .with_raw(raw)
}

/// Сбой HTTP-обмена (gateway/http.rs) с правдивой причиной.
/// Принимает: сбой, сроки, источник. Возвращает: GatewayError (Unreachable — соединение не установлено; Timeout —
/// запрос мог дойти, ответа целиком нет; BadResponse — тело не UTF-8 или больше предела).
pub fn from_fail(f: &Fail, lim: &Limits, source: &str) -> GatewayError {
    let code = match f {
        Fail::Connect(_) => ErrorCode::Unreachable,
        Fail::NotText | Fail::TooBig { .. } => ErrorCode::BadResponse,
        _ => ErrorCode::Timeout,
    };
    GatewayError::new(code, Some(venue()), format!("{source}: {}", f.text(lim)))
}

/// Ответ не удалось разобрать. Принимает: что не так, источник, сырой ответ. Возвращает: GatewayError BadResponse.
pub fn bad(what: &str, source: &str, raw: &str) -> GatewayError {
    GatewayError::new(
        ErrorCode::BadResponse,
        Some(venue()),
        format!("{source}: непонятный ответ: {what}"),
    )
    .with_raw(raw)
}

//! Файл: venues/bsc_dex/errors.rs
//! Что делает: перевод ответов узла BSC (JSON-RPC ошибка, HTTP-статус, сеть) в единые коды шлюза.
//! Кто вызывает: node/rpc.rs, node/ws.rs, позже send/* (каналы отправки — свои уточнения поверх этого).
//! От чего зависит: gateway/errors.rs, ctx.rs (ID).
//! Настройки: нет.
//!
//! Источники:
//! - формат ошибки JSON-RPC {code, message, data} — https://www.jsonrpc.org/specification#error_object
//!   (тот же формат в execution-apis: https://github.com/ethereum/execution-apis/blob/16caee274b0d4fb7a0316f294029adac88a75a09/src/eth/submit.yaml#L32-L71);
//! - узлы на практике отвечают -32000 + текст, разбирать надо по тексту (research/flow/bsc_send.md, раздел 1.2);
//! - тексты узла BSC: «nonce too low» — https://github.com/bnb-chain/bsc/blob/947ba1cb1ef05d4a5f71b738113a3d8b4c16f6b7/core/error.go#L51-L53 ;
//!   «already known», «replacement transaction underpriced», «transaction underpriced» —
//!   https://github.com/bnb-chain/bsc/blob/947ba1cb1ef05d4a5f71b738113a3d8b4c16f6b7/core/txpool/errors.go#L24-L38 ;
//!   «insufficient funds for gas * price + value» — https://github.com/bnb-chain/bsc/blob/947ba1cb1ef05d4a5f71b738113a3d8b4c16f6b7/core/error.go#L82 ;
//!   «execution reverted» (откат eth_call / eth_estimateGas) —
//!   https://github.com/bnb-chain/bsc/blob/947ba1cb1ef05d4a5f71b738113a3d8b4c16f6b7/core/vm/errors.go#L32 .
//!
//! Текст причины всегда берётся из ответа узла как есть — его видит Тигран.

use crate::gateway::errors::{ErrorCode, GatewayError};

use super::ctx::ID;
use crate::gateway::http::{Fail, Limits};

/// Какой запрос вернул ошибку: от этого зависит единый код у текстов, которые не распознаны.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    /// Чтение (eth_call, eth_getLogs, балансы, подписки).
    Read,
    /// Отправка подписанной транзакции.
    Send,
}

/// Ошибка из поля error ответа JSON-RPC.
/// Принимает: код, текст, источник (узел или канал), операцию, сырой ответ. Возвращает: GatewayError.
pub fn from_rpc(code: i64, message: &str, source: &str, op: Op, raw: &str) -> GatewayError {
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
        match op {
            Op::Send => ErrorCode::ChannelRejected,
            Op::Read => ErrorCode::BadResponse,
        }
    };
    GatewayError::new(
        unified,
        Some(venue()),
        format!("{source}, код {code}: {message}"),
    )
    .with_raw(raw)
}

/// Ошибка по HTTP-статусу, когда тело — не ответ JSON-RPC.
/// 401/403 — ключ провайдера не принят; 429 — превышен лимит запросов провайдера.
/// Принимает: статус, источник, сырой ответ. Возвращает: GatewayError.
pub fn from_http(status: u16, source: &str, raw: &str) -> GatewayError {
    let (code, text) = match status {
        401 | 403 => (
            ErrorCode::InvalidCredentials,
            "доступ не разрешён — проверьте ключ в адресе",
        ),
        429 => (ErrorCode::RateLimited, "превышен лимит запросов"),
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

/// Сбой HTTP-обмена (gateway/http.rs) с правдивой причиной: нет связи, нет ответа за срок, ответ не дочитан.
/// Принимает: сбой, сроки, источник. Возвращает: GatewayError (Timeout — срок; Unreachable — связь или обрыв тела).
pub fn from_fail(f: &Fail, lim: &Limits, source: &str) -> GatewayError {
    let code = match f {
        Fail::NotText => ErrorCode::BadResponse,
        f if f.is_timeout() => ErrorCode::Timeout,
        _ => ErrorCode::Unreachable,
    };
    GatewayError::new(code, Some(venue()), format!("{source}: {}", f.text(lim)))
}

/// Ответ не удалось разобрать.
/// Принимает: что именно не так, источник, сырой ответ. Возвращает: GatewayError BadResponse.
pub fn bad(what: &str, source: &str, raw: &str) -> GatewayError {
    GatewayError::new(
        ErrorCode::BadResponse,
        Some(venue()),
        format!("{source}: непонятный ответ: {what}"),
    )
    .with_raw(raw)
}

/// Ошибка с площадкой bsc_dex там, где нет окружения Ctx.
/// Принимает: код, текст. Возвращает: GatewayError.
pub fn err(code: ErrorCode, message: impl Into<String>) -> GatewayError {
    GatewayError::new(code, Some(venue()), message)
}

/// Идентификатор площадки для ошибок.
fn venue() -> crate::core::types::VenueId {
    crate::core::types::VenueId(ID.to_string())
}

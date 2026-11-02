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
//! - «missing trie node» (-32000) — узел не архивный и состояние запрошенного блока уже удалил (живая запись
//!   2026-10-25 с bsc-dataseed.bnbchain.org: все eth_call на одном блоке, ответ {"code":-32000,"message":"missing trie
//!   node"} начиная примерно через минуту после блока) — своя причина StaleData «чтение на закреплённом блоке опоздало»,
//!   а не общая ошибка узла.
//! - «… state … is not available» (-32000) — то же у узла PBSS (свой geth-bsc v1.8.1, решение 121, часть A4): состояния
//!   блока нет (у full-узла — только последние ~128 блоков). Исходники bnb-chain/bsc v1.8.1: чтение состояния
//!   eth_call/eth_getBalance — eth/api_backend.go:243-248 (StateAt, при ошибке — HistoricState, её ошибка и уходит в
//!   ответ); core/state/database_history.go:254-255 «historical state %x is not available»;
//!   triedb/pathdb/reader.go:175, 190, 223 «state %#x is not available»; core/blockchain_reader.go:524
//!   «state is not available» — https://github.com/bnb-chain/bsc/tree/v1.8.1 . Совпадение — по двум частям текста:
//!   «state» и «is not available».
//! - «exceed max topics» (-32602, «invalid argument 1: exceed max topics») — фильтр logs больше предела узла: больше 4
//!   позиций topics или больше 1000 вариантов в одной позиции (geth-bsc v1.8.1 eth/filters/api.go:46, 64-66, 836-838,
//!   860-861; обёртка «invalid argument N» — rpc/json.go:346, код -32602 — rpc/handler.go:608-610, rpc/errors.go:142-146).
//!   У нас варианты в одной позиции — PoolId пачки (dex.pools_per_sub) — в причину дописывается, что делать (часть A5).
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
    } else if m.contains("missing trie node") {
        return GatewayError::new(
            ErrorCode::StaleData,
            Some(venue()),
            format!(
                "{source}, код {code}: {message} — состояние блока у узла уже удалено (узел не архивный): чтение на \
                 закреплённом блоке опоздало, нужен более свежий блок"
            ),
        )
        .with_raw(raw);
    } else if m.contains("state") && m.contains("is not available") {
        return GatewayError::new(
            ErrorCode::StaleData,
            Some(venue()),
            format!(
                "{source}, код {code}: {message} — состояния этого блока у узла нет (узел не архивный, хранит \
                 только последние блоки): чтение на закреплённом блоке опоздало, нужен более свежий блок"
            ),
        )
        .with_raw(raw);
    } else if m.contains("exceed max topics") {
        return GatewayError::new(
            ErrorCode::BadResponse,
            Some(venue()),
            format!(
                "{source}, код {code}: {message} — узел не принимает в одной подписке больше 1000 PoolId \
                 (предел geth-bsc): уменьшите dex.pools_per_sub до 1000 или меньше"
            ),
        )
        .with_raw(raw);
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
/// Принимает: сбой, сроки, источник. Возвращает: GatewayError (Unreachable — соединение не установлено; Timeout —
/// запрос мог дойти, ответа целиком нет; BadResponse — тело не UTF-8 или больше предела).
pub fn from_fail(f: &Fail, lim: &Limits, source: &str) -> GatewayError {
    // Unreachable — только «соединение не установлено» (запрос точно не дошёл); дошёл, а ответа целиком нет —
    // Timeout: результат неизвестен, для отправки транзакции — «неясно» (send/mod.rs), не «не отправлено».
    let code = match f {
        Fail::Connect(_) => ErrorCode::Unreachable,
        Fail::NotText | Fail::TooBig { .. } => ErrorCode::BadResponse,
        _ => ErrorCode::Timeout,
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

//! Файл: venues/bsc_dex/send/check.rs
//! Что делает: «Проверить подключение» канала отправки (решение 21): настройки и ключ на месте, адрес отвечает по
//! JSON-RPC. Проверка без настоящей транзакции: канал получает заведомо пустую транзакцию («0x») своим методом
//! отправки и должен ответить ошибкой JSON-RPC (так у JetBuilder в документации: -39403 «Invalid raw transaction»).
//! Ответ-ошибка канала = «Подключено»; нет связи, HTTP 401/403/429, не JSON-RPC — точная причина сбоя.
//! Кто вызывает: mod.rs (контракт check_send_channel).
//! От чего зависит: send/ready.rs, send/channels.rs, ctx.rs.
//! Настройки: channel.* [канал]; секрет channel.<id>.key.
//!
//! Методы отправки — шапки файлов каналов; формат ошибки — https://www.jsonrpc.org/specification#error_object ;
//! research/flow/bsc_builders.md, раздел 3.3 (проверка JetBuilder неверной транзакцией).

use alloy_primitives::Bytes;

use crate::core::types::ChannelState;
use crate::gateway::contract::ConnectionCheck;
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::channels;
use super::ready;

/// Проверка канала. Возвращает: ConnectionCheck. Ошибки: NotConfigured — такого канала нет.
pub async fn check(ctx: &Ctx, id: &str) -> GatewayResult<ConnectionCheck> {
    let ch = channels::find(id).ok_or_else(|| {
        ctx.err(
            ErrorCode::NotConfigured,
            format!("Канала «{id}» нет у адаптера DEX"),
        )
    })?;
    let r = match ready::prepare(ctx, ch) {
        Ok(r) => r,
        Err(state) => {
            return Ok(ConnectionCheck {
                ok: false,
                details: describe(&state),
            });
        }
    };
    let out = match r.send(ctx, &[Bytes::new()], 0).await {
        Ok(()) => ConnectionCheck {
            ok: true,
            details: format!("Подключено: {} принял запрос", ch.title),
        },
        Err(e) if answered(e.code) => ConnectionCheck {
            ok: true,
            details: format!(
                "Подключено: {} ответил на пустую транзакцию ошибкой, как и должен — «{}»",
                ch.title, e.message
            ),
        },
        Err(e) => ConnectionCheck {
            ok: false,
            details: e.message,
        },
    };
    Ok(out)
}

/// Ошибка пришла от самого канала по JSON-RPC (а не от сети или HTTP).
fn answered(code: ErrorCode) -> bool {
    matches!(
        code,
        ErrorCode::ChannelRejected
            | ErrorCode::NonceConflict
            | ErrorCode::InsufficientBalance
            | ErrorCode::GasBelowChannelMin
            | ErrorCode::SwapReverted
    )
}

/// Понятная причина состояния канала.
fn describe(s: &ChannelState) -> String {
    match s {
        ChannelState::Ready => "Готов".into(),
        ChannelState::Disabled => "Канал выключен (channel.enabled)".into(),
        ChannelState::NoKey => "Нужен ключ канала — введите его в секретах (решение 41: без ключа канал не используется)".into(),
        ChannelState::NotConfigured { reason } => reason.clone(),
        ChannelState::Failing { reason } => reason.clone(),
    }
}

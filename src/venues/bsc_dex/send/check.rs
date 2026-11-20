//! Файл: venues/bsc_dex/send/check.rs
//! Что делает: «Проверить подключение» канала отправки (решение 21): настройки и ключ на месте, адрес отвечает по
//! JSON-RPC. Проверка без настоящей транзакции: канал получает заведомо пустую транзакцию («0x») своим методом
//! отправки и должен ответить ошибкой JSON-RPC (так у JetBuilder в документации: -39403 «Invalid raw transaction»).
//! Ответ-ошибка канала = «Подключено»; нет связи, HTTP 401/403/429, не JSON-RPC — точная причина сбоя.
//! Решение 436: у секвенсора Arbitrum с окном channel.cond_time_window_s проверка идёт условным методом
//! (eth_sendRawTransactionConditional, пустые условия); метод не принимается (−32601) — не «Подключено», причина.
//! К ответу дописан минимум цены газа канала с узла (ArbGasInfo, node/arb_min_gas.rs) и окно условия.
//! Кто вызывает: mod.rs (контракт check_send_channel).
//! От чего зависит: send/ready.rs, send/channels.rs, node/arb_min_gas.rs, ctx.rs.
//! Настройки: channel.* [канал]; секрет channel.<id>.key.
//!
//! Методы отправки — шапки файлов каналов; формат ошибки — https://www.jsonrpc.org/specification#error_object ;
//! research/flow/bsc_builders.md, раздел 3.3 (проверка JetBuilder неверной транзакцией).

use crate::core::types::ChannelState;
use crate::gateway::contract::ConnectionCheck;
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::node::arb_min_gas;
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
    let extra = node_part(ctx, &r);
    let out = match r.probe(ctx).await {
        Ok(()) => ConnectionCheck {
            ok: true,
            details: format!("Подключено: {} принял запрос{extra}", ch.title),
        },
        Err(e) if answered(e.code) => ConnectionCheck {
            ok: true,
            details: format!(
                "Подключено: {} ответил на пустую транзакцию ошибкой, как и должен — «{}»{extra}",
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

/// Решение 436: у канала с минимумом с узла — значение и блок (ArbGasInfo.getMinimumGasPrice), у канала с окном — окно
/// условия. Принимает: окружение, готовый канал. Возвращает: дописку к ответу (пусто у остальных каналов). Ошибок нет.
fn node_part(ctx: &Ctx, r: &ready::Ready) -> String {
    let mut out = String::new();
    if let (true, Some((block, wei))) = (r.ch.min_from_node(), arb_min_gas::last(ctx)) {
        out.push_str(&format!(
            "; минимум цены газа сети с узла (ArbGasInfo.getMinimumGasPrice): {wei} wei, блок {block}"
        ));
    }
    if let Some(w) = r.cond {
        out.push_str(&format!(
            "; условная отправка eth_sendRawTransactionConditional, окно {w} с (timestampMax)"
        ));
    }
    out
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

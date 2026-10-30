//! Файл: venues/bsc_dex/send/mod.rs
//! Что делает: отправка подписанной транзакции в сеть через каналы (решения 33, 41–45): выбор готовых каналов,
//! правила (открытый канал — только один и не вместе с закрытыми; несколько закрытых — параллельно и без чаевых),
//! нижняя граница цены газа по выбранным каналам, параллельная отправка одной и той же транзакции, счётчики.
//! Каждый канал — свой файл по своей официальной документации; общий вид — channel.rs, отправка — ready.rs.
//! Кто вызывает: exec/*, wallet/transfer.rs, mod.rs (send_channels, check_send_channel).
//! От чего зависит: файлы этой папки, ctx.rs.
//! Настройки: channel.* [канал], send.* [bsc_dex].
//!
//! Параллельная отправка одной подписанной транзакции официально нигде не запрещена; один хеш и один nonce — в блок
//! попадёт не больше одной копии (research/flow/bsc_send.md, раздел 4; bsc_builders.md, раздел 8). Чаевые — перевод
//! на адрес своего билдера, поэтому только при отправке в один канал (48 Club: бандл с переводом другому билдеру —
//! ошибка 4851).

pub mod blink;
pub mod blockrazor;
pub mod blockrazor_builder;
pub mod bloxroute_cloud;
pub mod bloxroute_protect;
pub mod cancel;
pub mod channel;
pub mod channels;
pub mod check;
pub mod club48;
pub mod flashblock;
pub mod jetbuilder;
pub mod mev_guard;
pub mod nodereal;
pub mod nodereal_api;
pub mod public;
pub mod ready;
pub mod receipt;
pub mod settle;
pub mod stats;
pub mod track;
pub mod track_util;
pub mod tx;

use alloy_primitives::{Bytes, U256};
use futures_util::future::join_all;

use crate::core::types::SendMethod;
use crate::gateway::errors::{ErrorCode, GatewayResult};
use crate::storage::journal::Level;

use super::ctx::Ctx;
use ready::Ready;
use stats::Stats;

/// Готовые каналы с проверкой правил решения 33. Ошибки: NotConfigured — нет ни одного готового канала или
/// открытый канал включён вместе с закрытыми / не один.
pub fn select(ctx: &Ctx, stats: &Stats) -> GatewayResult<Vec<Ready>> {
    let mut ready = Vec::new();
    for ch in channels::ALL {
        match ready::prepare(ctx, ch) {
            Ok(r) => ready.push(r),
            Err(state) => {
                if stats
                    .get(ch.id, crate::core::types::ChannelState::Ready)
                    .state
                    != state
                {
                    stats.update(ch.id, |s| s.state = state);
                }
            }
        }
    }
    let public = ready.iter().filter(|r| !r.ch.private).count();
    if public > 0 && ready.len() > 1 {
        return Err(ctx.err(ErrorCode::NotConfigured, "Открытый канал включён вместе с другими — по решению 33 открытая отправка только одна и не вместе с закрытыми"));
    }
    if ready.is_empty() {
        return Err(ctx.err(ErrorCode::NotConfigured, "Нет ни одного готового канала отправки (channel.enabled, адрес, способ, минимум газа, ключ)"));
    }
    Ok(ready)
}

/// Нижняя граница цены газа: наибольший минимум выбранных каналов (решение 44); каналов нет — границы нет (0).
/// Ошибок нет.
pub fn gas_floor(ready: &[Ready]) -> U256 {
    ready
        .iter()
        .map(|r| r.min_gas_wei)
        .fold(U256::ZERO, U256::max)
}

/// Чаевые разрешены: ровно один канал, бандл, чаевые включены. Возвращает адрес и сумму (wei).
pub fn tip(ready: &[Ready]) -> Option<(alloy_primitives::Address, U256)> {
    match ready {
        [r] if r.method == SendMethod::Bundle => r.tip,
        _ => None,
    }
}

/// Итог отправки в каналы.
#[derive(Debug, Clone, Default)]
pub struct Dispatched {
    /// Каналы, которые приняли транзакцию.
    pub accepted: Vec<String>,
    /// Каналы без ясного ответа (таймаут, обрыв, ответ не разобран): транзакция могла дойти и попасть в блок.
    pub uncertain: Vec<String>,
}

impl Dispatched {
    /// Все каналы, где транзакция может быть. Ошибок нет.
    pub fn all(&self) -> Vec<String> {
        self.accepted
            .iter()
            .chain(&self.uncertain)
            .cloned()
            .collect()
    }
}

/// Отправляет во все выбранные каналы параллельно. Принимает: каналы, подписанную транзакцию, перевод чаевых
/// (только при одном канале), предел блока бандла. Возвращает: кто принял и кто не ответил ясно.
/// Ошибки: ChannelRejected — все каналы ответили отказом (транзакции точно нигде нет; причины всех каналов в тексте).
pub async fn dispatch(
    ctx: &Ctx,
    stats: &Stats,
    ready: &[Ready],
    tx: &Bytes,
    tip_tx: Option<&Bytes>,
    max_block: u64,
) -> GatewayResult<Dispatched> {
    let jobs = ready.iter().map(|r| {
        let mut txs = vec![tx.clone()];
        if let (SendMethod::Bundle, Some(t)) = (r.method, tip_tx) {
            txs.push(t.clone());
        }
        async move { (r.ch.id, r.send(ctx, &txs, max_block).await) }
    });
    classify(ctx, stats, join_all(jobs).await).await
}

/// Отправляет весь бандл из нескольких транзакций (решение 84) во все выбранные каналы параллельно. Все каналы должны
/// быть способны на бандл (channel.method = bundle): одиночный канал взял бы только первую транзакцию и потерял
/// остальные. Транзакции идут как есть, по порядку (nonce уже проставлен), revertingTxHashes пуст (всё-или-ничего —
/// eth_sendBundle без разрешения на откат). Принимает: каналы, подписанные транзакции бандла по порядку, предел блока.
/// Возвращает: кто принял и кто не ответил ясно. Ошибки: NotConfigured (есть не-бандл канал), ChannelRejected (никто).
pub async fn dispatch_slice(
    ctx: &Ctx,
    stats: &Stats,
    ready: &[Ready],
    txs: &[Bytes],
    max_block: u64,
) -> GatewayResult<Dispatched> {
    if let Some(r) = ready.iter().find(|r| r.method != SendMethod::Bundle) {
        return Err(ctx.err(
            ErrorCode::NotConfigured,
            format!(
                "Канал {} не в режиме бандла (channel.method): мульти-tx бандл через него не отправить",
                r.ch.title
            ),
        ));
    }
    let jobs = ready
        .iter()
        .map(|r| async move { (r.ch.id, r.send(ctx, txs, max_block).await) });
    classify(ctx, stats, join_all(jobs).await).await
}

/// Разбор итогов отправки по каналам: счётчики, журнал, кто принял и кто не ответил ясно. Ошибки: ChannelRejected —
/// ни один канал не принял (транзакции точно нигде нет; причины всех каналов в тексте).
async fn classify(
    ctx: &Ctx,
    stats: &Stats,
    results: Vec<(&'static str, GatewayResult<()>)>,
) -> GatewayResult<Dispatched> {
    let mut out = Dispatched::default();
    let mut why = Vec::new();
    for (id, res) in results {
        match res {
            Ok(()) => {
                stats.update(id, |s| s.sent += 1);
                out.accepted.push(id.to_string());
            }
            Err(e) => {
                stats.update(id, |s| {
                    s.rejected += 1;
                    s.state = crate::core::types::ChannelState::Failing {
                        reason: e.message.clone(),
                    };
                });
                ctx.log(
                    Level::Warn,
                    "send.channel_rejected",
                    format!("{id}: {}", e.message),
                    None,
                )
                .await;
                // Нет ясного ответа — запрос мог дойти до канала: транзакция считается отправленной.
                if matches!(
                    e.code,
                    ErrorCode::Timeout | ErrorCode::Unreachable | ErrorCode::BadResponse
                ) {
                    out.uncertain.push(id.to_string());
                }
                why.push(format!("{id}: {}", e.message));
            }
        }
    }
    if out.accepted.is_empty() && out.uncertain.is_empty() {
        return Err(ctx.err(
            ErrorCode::ChannelRejected,
            format!("Ни один канал не принял транзакцию: {}", why.join("; ")),
        ));
    }
    Ok(out)
}

/// Чаевые, которые будут заплачены при текущих настройках каналов (для расчёта затрат, решение 35):
/// только если готов ровно один канал, бандлом и с включёнными чаевыми. Возвращает: сумму в wei. Ошибок нет.
pub fn planned_tip(ctx: &Ctx) -> Option<U256> {
    let ready: Vec<Ready> = channels::ALL
        .iter()
        .filter_map(|c| ready::prepare(ctx, c).ok())
        .collect();
    tip(&ready).map(|(_, amount)| amount)
}

/// Нижняя граница цены газа по каналам, готовым сейчас (для расчёта затрат: газ в котировке не ниже того, с которым
/// транзакция реально уйдёт). Ошибки: NotConfigured — нет ни одного готового канала.
pub fn planned_floor(ctx: &Ctx) -> GatewayResult<U256> {
    let ready: Vec<Ready> = channels::ALL
        .iter()
        .filter_map(|c| ready::prepare(ctx, c).ok())
        .collect();
    if ready.is_empty() {
        return Err(ctx.err(
            ErrorCode::NotConfigured,
            "Нет ни одного готового канала отправки — газ сделки не считается",
        ));
    }
    Ok(gas_floor(&ready))
}

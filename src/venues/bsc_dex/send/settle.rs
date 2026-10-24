//! Файл: venues/bsc_dex/send/settle.rs
//! Что делает: очередь кошелька после итога транзакции (решение 45): исход не установлен — очередь остановлена до
//! ручной проверки (событие WalletQueue → Telegram send.queue_halted у ядра, журнал); иначе следующий nonce
//! перечитывается у узла с повтором на каждом блоке (не «nonce + 1» вслепую: бандл с чаевыми занимает и nonce + 1,
//! узел HTTP может отставать) — не перечитан за send.queue_max_wait_s — тоже остановка. Затем очередь свободна.
//! Кто вызывает: exec/txs.rs (задача трекера).
//! От чего зависит: wallet/nonce.rs (Hold), send/track_util.rs (Tracked, Quiet), send/stats.rs, send/tx.rs, node/rpc.rs.
//! Настройки: send.queue_max_wait_s, feed.stale_after_ms (SendCfg).
//!
//! eth_getTransactionCount «latest» — execution-apis 16caee27 src/eth/state.yaml.

use std::time::Instant;

use alloy_primitives::{Address, B256, U64};
use serde_json::json;

use crate::core::types::{WalletQueue, WalletQueueState};
use crate::gateway::events::MarketEvent;
use crate::storage::journal::Level;

use super::super::ctx::Ctx;
use super::super::node::heads::Heads;
use super::super::node::rpc;
use super::super::wallet::nonce::Hold;
use super::super::wallet::queue_store;
use super::stats::Stats;
use super::track_util::{Quiet, Tracked, next_head};
use super::tx::SendCfg;

/// Что нужно для решения об очереди.
pub struct After<'a> {
    pub ctx: &'a Ctx,
    pub heads: &'a Heads,
    pub stats: &'a Stats,
    pub cfg: &'a SendCfg,
    pub from: Address,
    pub nonce: u64,
    pub hash: B256,
}

/// Транзакция ушла: очередь ждёт её итога (SQLite, событие ядру). Ошибок нет.
pub async fn busy(a: &After<'_>, hold: &Hold) {
    emit(a, hold.busy(a.nonce, a.hash)).await;
}

/// Итог есть: очередь свободна (nonce перечитан) или остановлена до ручной проверки. Ошибок нет.
pub async fn settle(a: &After<'_>, hold: &Hold, t: &Tracked) {
    if let Some(why) = &t.unresolved {
        return halt(
            a,
            hold,
            format!("исход транзакции {} неизвестен: {why}", a.hash),
        )
        .await;
    }
    let started = Instant::now();
    let mut rx = a.heads.watch();
    let mut quiet = Quiet::new(a.ctx, a.hash);
    loop {
        match rpc::call::<U64>(a.ctx, "eth_getTransactionCount", json!([a.from, "latest"])).await {
            Ok(n) if n.to::<u64>() > a.nonce => {
                hold.consumed(n.to());
                emit(a, hold.free()).await;
                return;
            }
            Ok(_) => {
                quiet
                    .warn("lag", "узел ещё не видит nonce занятым".into())
                    .await
            }
            Err(e) => {
                quiet
                    .warn(
                        "nonce",
                        format!("nonce кошелька не прочитан: {}", e.message),
                    )
                    .await
            }
        }
        if started.elapsed() > a.cfg.queue_max_wait {
            let why = format!(
                "после итога {} nonce кошелька не перечитан за send.queue_max_wait_s: {}",
                a.hash,
                quiet.reason()
            );
            return halt(a, hold, why).await;
        }
        let _ = next_head(&mut rx, a.cfg, &mut quiet).await;
    }
}

/// Останавливает очередь до ручной проверки: журнал (ошибка) и событие ядру. Ошибок нет.
async fn halt(a: &After<'_>, hold: &Hold, why: String) {
    a.ctx
        .log(
            Level::Error,
            "send.queue_halted",
            format!("Очередь кошелька остановлена до ручной проверки: {why}"),
            None,
        )
        .await;
    emit(a, hold.halt(why, a.nonce, a.hash)).await;
}

/// Сохраняет состояние в SQLite (переживает перезапуск) и шлёт событие WalletQueue ядру. Ошибок нет.
async fn emit(a: &After<'_>, state: WalletQueueState) {
    queue_store::save(a.ctx, &state).await;
    a.stats.emit(MarketEvent::WalletQueue(WalletQueue {
        venue: a.ctx.venue(),
        state,
        nonce_taken: None,
    }));
}

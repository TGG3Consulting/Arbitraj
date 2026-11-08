//! Файл: venues/bsc_dex/send/settle.rs
//! Что делает: очередь кошелька после итога транзакции (решение 45): исход не установлен — очередь остановлена до
//! ручной проверки (событие WalletQueue → Telegram send.queue_halted у ядра, журнал); иначе следующий nonce
//! перечитывается у узла с повтором на каждом блоке (не «nonce + 1» вслепую: бандл с чаевыми занимает и nonce + 1,
//! узел HTTP может отставать) — не перечитан за send.queue_max_wait_s — тоже остановка. Затем очередь свободна.
//! Задача трекера прервана (паника внутри неё или отмена задачи tokio) — исход транзакции не установлен: очередь
//! ОСТАНОВЛЕНА («отслеживание транзакции <хеш> прервано: …»), а не потеряна вместе с задачей (guarded — внутри задачи,
//! пока она держит Hold: ловит панику и тогда, когда вызывающий уже прерван; joined — у вызывающего: задача упала мимо
//! guarded или отменена). Никогда не освобождаем очередь, не зная исхода; вызывающий отдаёт «исход неизвестен» с «чем
//! найти» (exec/txs.rs, exec/bundle_send.rs), исход выясняет ядро (решения 231/233).
//! Кто вызывает: exec/txs.rs (задача трекера одиночной), exec/bundle_send.rs (задача трекера бандла — settle_bundle);
//! guarded / joined — те же два места.
//! От чего зависит: wallet/nonce.rs (Hold), send/track_util.rs (Tracked, Quiet), send/stats.rs, send/tx.rs, node/rpc.rs,
//! futures-util (catch_unwind), tokio (JoinHandle).
//! Настройки: send.queue_max_wait_s, feed.stale_after_ms (SendCfg).
//!
//! eth_getTransactionCount «latest» — execution-apis 16caee27 src/eth/state.yaml.

use std::any::Any;
use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::time::Instant;

use alloy_primitives::{Address, B256, U64};
use futures_util::FutureExt;
use serde_json::json;
use tokio::task::{JoinError, JoinHandle};

use crate::core::types::{TxOutcome, WalletQueue, WalletQueueState};
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

/// Очередь после итога бандла (решение 84, приватный all-or-nothing). Отличие от settle(): «не попал в блок»
/// (NotIncluded, исход УСТАНОВЛЕН) для бандла — это НЕзанятый nonce (газ не потрачен, решения 40/45): очередь свободна
/// сразу, следующая транзакция берёт тот же первый nonce — БЕЗ ожидания роста eth_getTransactionCount (он не вырастет)
/// и БЕЗ остановки по таймауту (ревью #82: иначе любое штатное истечение бандла халтило бы всю торговлю кошелька,
/// включая классику). Исход неизвестен (узел молчал) → halt, как обычно. Остальное (в блоке / откат в блоке, nonce
/// занят) — через settle(). Ошибок нет.
pub async fn settle_bundle(a: &After<'_>, hold: &Hold, t: &Tracked) {
    if t.unresolved.is_none() && t.outcome == TxOutcome::NotIncluded {
        // nonce не заняты — ничего не «consumed», очередь просто свободна.
        emit(a, hold.free()).await;
        return;
    }
    settle(a, hold, t).await;
}

/// Что делает: работа задачи трекера под защитой от паники. Паника внутри `work` (квитанции, settle, уведомления) —
/// очередь остановлена («отслеживание транзакции <хеш> прервано: паника задачи трекера: …») удержанием `hold`, пока
/// оно ещё живо: другая транзакция кошелька место взять не успеет. Принимает: решение об очереди (хеш и nonce, которыми
/// она остановится), удержание, работу. Возвращает: Ok(итог работы); Err(причина) — работа прервана паникой, исход не
/// установлен. Ошибок своих нет.
pub async fn guarded<T>(
    a: &After<'_>,
    hold: &Hold,
    work: impl Future<Output = T>,
) -> Result<T, String> {
    match AssertUnwindSafe(work).catch_unwind().await {
        Ok(v) => Ok(v),
        Err(p) => Err(aborted(
            a,
            hold,
            format!("паника задачи трекера: {}", panic_text(p.as_ref())),
        )
        .await),
    }
}

/// Что делает: итог задачи трекера у вызывающего. Задача упала мимо guarded (паника) или отменена — очередь остановлена
/// удержанием вызывающего (его Slot ещё жив). Принимает: решение об очереди, удержание, задачу (её итог — из guarded).
/// Возвращает: Ok(итог); Err(причина) — исход не установлен. Ошибок своих нет.
pub async fn joined<T>(
    a: &After<'_>,
    hold: &Hold,
    job: JoinHandle<Result<T, String>>,
) -> Result<T, String> {
    match job.await {
        Ok(r) => r,
        Err(e) => Err(aborted(a, hold, join_text(e)).await),
    }
}

/// Отслеживание прервано: очередь остановлена с причиной «отслеживание транзакции <хеш> прервано: <что>». Возвращает:
/// причину. Ошибок нет.
async fn aborted(a: &After<'_>, hold: &Hold, what: String) -> String {
    let why = format!("отслеживание транзакции {} прервано: {what}", a.hash);
    halt(a, hold, why.clone()).await;
    why
}

/// Текст паники (&str или String — так паникуют panic!/expect); иное — «без текста». Ошибок нет.
fn panic_text(p: &(dyn Any + Send)) -> String {
    p.downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| p.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "без текста".into())
}

/// Текст сбоя задачи tokio: паника (с текстом) или отмена. Ошибок нет.
fn join_text(e: JoinError) -> String {
    if e.is_panic() {
        format!(
            "паника задачи трекера: {}",
            panic_text(e.into_panic().as_ref())
        )
    } else {
        format!("задача трекера отменена ({e})")
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
        account: a.ctx.account.clone().map(crate::core::types::DexAccountId),
        state,
        nonce_taken: None,
    }));
}

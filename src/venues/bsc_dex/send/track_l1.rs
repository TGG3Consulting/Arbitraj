//! Файл: venues/bsc_dex/send/track_l1.rs
//! Что делает: решение 437 (1) — итог транзакции в сети с dex.gas_mode = eip1559_l1 (Arbitrum One) БЕЗ отмен.
//! Пула транзакций у сети нет, невключённую транзакцию секвенсор выбрасывает сам, а «пустая транзакция себе с тем же
//! nonce» либо сожжёт газ (исходная уже выброшена), либо получит «nonce too low» (исходная в блоке) — вытеснять нечего
//! (/home/claude/src-ext/reports/arb-cancel.md, раздел «Как правильно сделать в боте», п. 1). Поэтому:
//! - квитанция своим узлом на каждый новый блок (как send/track.rs) — «в блоке» (status 0x1) или «откат» (0x0);
//! - квитанции нет дольше send.drop_wait_ms ПО ВРЕМЕНИ от начала слежения (блоки Arbitrum идут только при
//!   транзакциях — arbitrum-docs how-arbitrum-works/deep-dives/sequencer-transaction-flow.mdx:95, поэтому не по
//!   блокам): eth_getTransactionCount(«latest») — nonce не занят → «выброшена» (NotIncluded без отмены: nonce
//!   свободен, сделка закрывается без повтора — правило «не исполнился — отменяем, больше не пробуем»; очередь
//!   кошелька свободна сразу, send/settle.rs); занят → ещё send.replaced_confirm_blocks блоков ищем свою квитанцию и
//!   eth_getTransactionByHash, потом «вытеснена» (как send/track.rs);
//!   Ревью 437, п. 1: «выброшена» — только по свежему своему узлу: голова получена не раньше feed.stale_after_ms
//!   назад (SendCfg.stale) и время её блока не раньше начала слежения + send.drop_wait_ms; иначе узел мог отстать от
//!   секвенсора (квитанции нет и nonce не вырос — у отставшего узла, а транзакция в блоке) — ждём дальше;
//! - исход не установлен за send.queue_max_wait_s — «исход неизвестен» (очередь останавливается).
//!
//! Ответ отправки уже разобран до трекера (send/mod.rs classify): ошибка всех каналов и своего узла нет транзакции —
//! ChannelRejected (не исполнена, nonce свободен, решение 154); «nonce too low» — та же проверка своим узлом по хешу
//! (наша в блоке — к трекеру, иначе отказ); −32003 условной отправки — отказ (решение 436); таймаут/обрыв — «без
//! ясного ответа» → сюда, ждать по времени. «"pending" = "latest"» у Arbitrum — arbitrum-docs
//! arbitrum-essentials/arbitrum-vs-ethereum/nonce-management.mdx:64; пула нет — nitro-geth arbitrum/apibackend.go:729-745.
//! Решение 398: все настройки — в SendCfg до отправки (drop_wait, replaced_confirm, queue_max_wait, stale); здесь не
//! читаются (сторож tests/settings_before_send_guard.rs).
//! Кто вызывает: send/track.rs (resolve при SendCfg.drop_wait).
//! От чего зависит: node/{rpc, heads, head_age}.rs, send/receipt.rs, send/track_util.rs, ctx.rs.
//! Настройки: нет своих чтений (send.drop_wait_ms и прочие — через SendCfg).

use std::time::{Duration, Instant};

use alloy_primitives::U64;
use chrono::Utc;
use serde_json::json;

use crate::core::types::TxOutcome;
use crate::storage::journal::Level;

use super::super::node::heads::Head;
use super::super::node::{head_age, rpc};
use super::receipt;
use super::track_util::{Quiet, Tracked, Watch, heads_rx, landed};

/// Ждёт итога без отмен (см. шапку). Принимает: что отслеживается, окно send.drop_wait_ms. Возвращает: итог.
/// Ошибок нет — сбои узла повторяются.
pub async fn resolve(w: Watch, window: Duration) -> Tracked {
    let ctx = &w.ctx;
    let started = Instant::now();
    let started_ms = Utc::now().timestamp_millis();
    let mut rx = heads_rx(&w.heads);
    let mut quiet = Quiet::new(ctx, w.hash);
    let ours = [w.hash];
    let mut taken_at: Option<u64> = None;
    loop {
        if started.elapsed() > w.cfg.queue_max_wait {
            let why = format!(
                "итог не установлен за send.queue_max_wait_s; последний сбой: {}",
                quiet.reason()
            );
            return Tracked::bare(TxOutcome::Pending, Some(why));
        }
        let head = step(&mut rx, &w, started, window).await;
        match receipt::find(ctx, &ours).await {
            Ok(Some((i, r))) => return landed(&w, &ours, i, r),
            Ok(None) => quiet.ok("receipt").await,
            Err(e) => {
                let why = format!("квитанция не прочитана: {}", e.message);
                quiet.warn("receipt", why).await;
                continue;
            }
        }
        if started.elapsed() < window {
            continue;
        }
        if let Some(at) = taken_at {
            // Nonce занят, своей квитанции нет: узел знает нашу транзакцию в блоке — ждём квитанцию дальше.
            match receipt::in_block(ctx, &ours).await {
                Ok(true) => continue,
                Ok(false) if head >= at + w.cfg.replaced_confirm => {
                    return Tracked::bare(TxOutcome::Replaced, None);
                }
                Ok(false) => continue,
                Err(e) => {
                    let why = format!("eth_getTransactionByHash: {}", e.message);
                    quiet.warn("by_hash", why).await;
                    continue;
                }
            }
        }
        let count: U64 =
            match rpc::call(ctx, "eth_getTransactionCount", json!([w.from, "latest"])).await {
                Ok(c) => c,
                Err(e) => {
                    let why = format!("nonce кошелька не прочитан: {}", e.message);
                    quiet.warn("nonce", why).await;
                    continue;
                }
            };
        if count.to::<u64>() > w.nonce {
            taken_at = Some(head);
            continue;
        }
        match caught_up(&w, started_ms, window).await {
            Ok(true) => {}
            Ok(false) => continue,
            Err(why) => {
                quiet.warn("head_time", why).await;
                continue;
            }
        }
        let msg = format!(
            "{} без квитанции за send.drop_wait_ms ({} мс), nonce {} не занят — выброшена секвенсором, \
             не исполнена (отмен у eip1559_l1 нет, повтора нет)",
            w.hash,
            window.as_millis(),
            w.nonce
        );
        ctx.log(Level::Warn, "send.tx_not_included", msg, None)
            .await;
        return Tracked::bare(TxOutcome::NotIncluded, None);
    }
}

/// Шаг ожидания: новый блок или конец окна send.drop_wait_ms — что раньше, но не дольше feed.stale_after_ms (SendCfg).
/// Поток блоков закрыт — сон того же шага (без пустого круга). Принимает: приёмник голов, слежение, начало, окно.
/// Возвращает: номер последней известной головы (0 — голов ещё не было). Ошибок нет.
async fn step(
    rx: &mut tokio::sync::watch::Receiver<Option<Head>>,
    w: &Watch,
    started: Instant,
    window: Duration,
) -> u64 {
    let left = window.saturating_sub(started.elapsed());
    let wait = if left.is_zero() {
        w.cfg.stale
    } else {
        left.min(w.cfg.stale)
    };
    if let Ok(Err(_)) = tokio::time::timeout(wait, rx.changed()).await {
        tokio::time::sleep(wait).await;
    }
    w.heads.latest().map_or(0, |h| h.number)
}

/// Ревью 437, п. 1: свой узел догнал секвенсор настолько, чтобы верить «квитанции нет, nonce не вырос»: голова свежая
/// (получена не раньше SendCfg.stale назад) и время её блока (секунды, с округлением вниз) не раньше начала слежения +
/// окно. Принимает: слежение, начало (мс Unix), окно send.drop_wait_ms. Возвращает: да/нет.
/// Ошибки: текст — время блока головы не прочитано (узел).
async fn caught_up(w: &Watch, started_ms: i64, window: Duration) -> Result<bool, String> {
    let Some(h) = w.heads.latest() else {
        return Ok(false);
    };
    let age = (Utc::now() - h.received_at).num_milliseconds();
    let stale = i64::try_from(w.cfg.stale.as_millis()).unwrap_or(i64::MAX);
    if !(0..=stale).contains(&age) {
        return Ok(false);
    }
    let secs = match w.heads.block_time().filter(|(n, _)| *n == h.number) {
        Some((_, t)) => t,
        None => head_age::fetch(&w.ctx, h.number, true)
            .await
            .map_err(|e| format!("время блока головы {}: {}", h.number, e.message))?
            .secs,
    };
    let need = started_ms.saturating_add(i64::try_from(window.as_millis()).unwrap_or(i64::MAX));
    Ok(i64::try_from(secs).unwrap_or(i64::MAX).saturating_mul(1000) >= need)
}

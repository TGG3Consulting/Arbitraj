//! Файл: venues/bsc_dex/send/track.rs
//! Что делает: итог транзакции — только через свой узел (решение 42), независимо от канала, и до тех пор, пока её
//! nonce не занят в блоке. На каждый новый блок (опрос блоков, решение 57) — eth_getTransactionReceipt по исходной транзакции и её
//! отменам. Квитанция исходной — «в блоке» (status 0x1) или «откат» (0x0); квитанция отмены — «не попала».
//! Нет квитанции дольше send.max_wait_blocks: nonce («latest») не занят — отмена (send/cancel.rs), дороже каждый раз;
//! занят — ещё send.replaced_confirm_blocks блоков ищем свою квитанцию и eth_getTransactionByHash (узел мог ещё не
//! проиндексировать, балансировщик RPC), только потом «вытеснена». Сбой узла, нет блоков — не ошибка: повтор.
//! Исход не установлен: лимит отмен (send.cancel_max_attempts), потолок цены газа отмены (send.cancel_max_gas_gwei),
//! время ожидания (send.queue_max_wait_s) — итог «исход неизвестен» (exec/txs.rs останавливает очередь).
//! Кто вызывает: exec/txs.rs (задача трекера держит место в очереди кошелька).
//! От чего зависит: node/rpc.rs, send/track_util.rs, send/receipt.rs, send/cancel.rs, send/tx.rs (SendCfg), ctx.rs.
//! Настройки: send.max_wait_blocks, send.replaced_confirm_blocks, send.cancel_*, send.queue_max_wait_s,
//! send.bundle_max_blocks, feed.stale_after_ms (SendCfg).
//!
//! Источники: квитанция — execution-apis 16caee27 src/eth/transaction.yaml#L127-L165; null, пока нет в блоке — узел BSC
//! internal/ethapi/api.go#L2211-L2229; eth_getTransactionCount «latest» — src/eth/state.yaml; схема трекера —
//! research/flow/bsc_send.md, раздел 5.

use std::time::Instant;

use alloy_primitives::U64;
use serde_json::json;

use crate::core::types::TxOutcome;
use crate::storage::journal::Level;

use super::super::node::rpc;
use super::cancel;
use super::receipt;
pub use super::receipt::Receipt;
use super::track_util::{Quiet, heads_rx, landed, next_head};
pub use super::track_util::{Tracked, Watch};

/// Ждёт, пока nonce не занят в блоке или исход не признан неизвестным (см. шапку). Возвращает: итог.
/// Ошибок нет — сбои узла повторяются.
pub async fn resolve(w: Watch) -> Tracked {
    let ctx = &w.ctx;
    let started = Instant::now();
    let mut rx = heads_rx(&w.heads);
    let mut quiet = Quiet::new(ctx, w.hash);
    let mut ours = vec![w.hash];
    let (mut price, mut since, mut cancels) = (w.price, w.sent_block, 0u64);
    let mut taken_at: Option<u64> = None;
    loop {
        if started.elapsed() > w.cfg.queue_max_wait {
            let why = format!(
                "итог не установлен за send.queue_max_wait_s; последний сбой: {}",
                quiet.reason()
            );
            return Tracked::bare(TxOutcome::Pending, Some(why));
        }
        let Some(head) = next_head(&mut rx, &w.cfg, &mut quiet).await else {
            continue;
        };
        match receipt::find(ctx, &ours).await {
            Ok(Some((i, r))) => return landed(&w, &ours, i, r),
            Ok(None) => quiet.ok("receipt").await,
            Err(e) => {
                quiet
                    .warn("receipt", format!("квитанция не прочитана: {}", e.message))
                    .await;
                continue;
            }
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
                    quiet
                        .warn(
                            "by_hash",
                            format!("eth_getTransactionByHash: {}", e.message),
                        )
                        .await;
                    continue;
                }
            }
        }
        if head <= since + w.cfg.max_wait {
            continue;
        }
        let count: U64 =
            match rpc::call(ctx, "eth_getTransactionCount", json!([w.from, "latest"])).await {
                Ok(c) => c,
                Err(e) => {
                    quiet
                        .warn(
                            "nonce",
                            format!("nonce кошелька не прочитан: {}", e.message),
                        )
                        .await;
                    continue;
                }
            };
        if count.to::<u64>() > w.nonce {
            taken_at = Some(head);
            continue;
        }
        since = head;
        if cancels >= w.cfg.cancel_max_attempts {
            let why = format!(
                "отмен уже {cancels} (send.cancel_max_attempts), nonce {} не занят",
                w.nonce
            );
            return Tracked::bare(TxOutcome::Pending, Some(why));
        }
        price = match cancel::price(ctx, price, &w.cfg, &w.ready) {
            Ok(p) if p <= w.cfg.cancel_max_wei => p,
            Ok(p) => {
                let why =
                    format!("цена газа следующей отмены {p} wei выше send.cancel_max_gas_gwei");
                return Tracked::bare(TxOutcome::Pending, Some(why));
            }
            Err(e) => return Tracked::bare(TxOutcome::Pending, Some(e.message)),
        };
        cancels += 1;
        match cancel::send(
            ctx, &w.stats, &w.ready, w.from, w.nonce, price, head, &w.cfg,
        )
        .await
        {
            Ok(h) => {
                ours.push(h);
                quiet.ok("cancel").await;
                let msg = format!(
                    "{} не попала за send.max_wait_blocks — отмена {h} (nonce {}, {price} wei)",
                    w.hash, w.nonce
                );
                ctx.log(Level::Warn, "send.tx_not_included", msg, None)
                    .await;
            }
            Err(e) => {
                let msg = format!(
                    "отмена (nonce {}, {price} wei) не ушла: {}",
                    w.nonce, e.message
                );
                quiet.warn("cancel", msg).await;
            }
        }
    }
}

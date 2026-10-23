//! Файл: venues/bsc_dex/send/track.rs
//! Что делает: итог транзакции — только через свой узел (решение 42), независимо от канала, и до тех пор, пока её
//! nonce не занят в блоке. На каждый новый блок (newHeads) — eth_getTransactionReceipt по исходной транзакции и её
//! отменам. Квитанция исходной — «в блоке» (status 0x1) или «откат» (0x0); квитанция отмены — «не попала» (газ
//! отмены — в факте). Нет квитанции дольше send.max_wait_blocks: nonce кошелька («latest») занят — ещё раз квитанции,
//! нет — «вытеснена» (чужая транзакция с тем же ключом); не занят — отмена (send/cancel.rs) и снова ждём.
//! Сбой чтения квитанции или nonce, нет новых блоков — не ошибка: повтор на следующем блоке, очередь ждёт.
//! Кто вызывает: exec/txs.rs (задача трекера держит место в очереди кошелька).
//! От чего зависит: node/rpc.rs, node/heads.rs, send/receipt.rs, send/cancel.rs, send/stats.rs, send/tx.rs (SendCfg), ctx.rs.
//! Настройки: send.max_wait_blocks, send.cancel_gas_bump_pct, send.bundle_max_blocks, feed.stale_after_ms (SendCfg).
//!
//! Источники: квитанция (status, blockNumber, gasUsed, effectiveGasPrice, logs) —
//! https://github.com/ethereum/execution-apis/blob/16caee274b0d4fb7a0316f294029adac88a75a09/src/eth/transaction.yaml#L127-L165 ;
//! null, пока нет в блоке — узел BSC internal/ethapi/api.go#L2211-L2229; eth_getTransactionCount «latest» —
//! execution-apis src/eth/state.yaml; схема трекера — research/flow/bsc_send.md, раздел 5.

use alloy_primitives::{Address, B256, U64, U256};
use serde_json::json;

use crate::core::types::TxOutcome;
use crate::storage::journal::Level;

use super::super::ctx::Ctx;
use super::super::node::heads::Heads;
use super::super::node::rpc;
use super::cancel;
use super::ready::Ready;
use super::receipt;
pub use super::receipt::Receipt;
use super::stats::Stats;
use super::tx::SendCfg;

/// Итог: nonce занят в блоке.
pub struct Tracked {
    pub outcome: TxOutcome,
    /// Квитанция исходной транзакции (в блоке / откат).
    pub receipt: Option<Receipt>,
    /// Отмена, занявшая nonce: хеш и квитанция (исходная не попала).
    pub cancel: Option<(B256, Receipt)>,
}

/// Что отслеживать.
pub struct Watch {
    pub ctx: Ctx,
    pub heads: Heads,
    pub stats: Stats,
    /// Каналы исходной транзакции — в них же уходит отмена.
    pub ready: Vec<Ready>,
    pub cfg: SendCfg,
    pub hash: B256,
    pub nonce: u64,
    pub from: Address,
    /// Цена газа исходной транзакции, wei.
    pub price: U256,
    pub sent_block: u64,
    /// Каналы, принявшие (или не ответившие ясно) — для счётчиков.
    pub channels: Vec<String>,
}

/// Ждёт, пока nonce не занят в блоке (см. шапку). Возвращает: итог. Ошибок нет — сбои узла повторяются.
pub async fn resolve(w: Watch) -> Tracked {
    let ctx = &w.ctx;
    let mut rx = w.heads.watch();
    let mut ours = vec![w.hash];
    let (mut price, mut since) = (w.price, w.sent_block);
    loop {
        match tokio::time::timeout(w.cfg.stale, rx.changed()).await {
            Ok(Ok(())) => {}
            Ok(Err(_)) => {
                warn(ctx, format!("поток новых блоков закрыт — {} ждёт", w.hash)).await;
                tokio::time::sleep(w.cfg.stale).await;
                continue;
            }
            Err(_) => {
                warn(ctx, format!("нет новых блоков дольше feed.stale_after_ms — итог {} пока неизвестен, очередь кошелька ждёт", w.hash)).await;
                continue;
            }
        }
        let Some(head) = rx.borrow_and_update().clone() else {
            continue;
        };
        match receipt::find(ctx, &ours).await {
            Ok(Some((i, r))) => return landed(&w, &ours, i, r),
            Ok(None) => {}
            Err(e) => {
                warn(
                    ctx,
                    format!(
                        "квитанция {}: {} — повтор на следующем блоке",
                        w.hash, e.message
                    ),
                )
                .await;
                continue;
            }
        }
        if head.number <= since + w.cfg.max_wait {
            continue;
        }
        let count: U64 =
            match rpc::call(ctx, "eth_getTransactionCount", json!([w.from, "latest"])).await {
                Ok(c) => c,
                Err(e) => {
                    warn(
                        ctx,
                        format!("nonce кошелька: {} — повтор на следующем блоке", e.message),
                    )
                    .await;
                    continue;
                }
            };
        if count.to::<u64>() > w.nonce {
            // Nonce занят: квитанция нашей транзакции могла появиться между чтениями — читаем ещё раз.
            match receipt::find(ctx, &ours).await {
                Ok(Some((i, r))) => return landed(&w, &ours, i, r),
                Ok(None) => {
                    return Tracked {
                        outcome: TxOutcome::Replaced,
                        receipt: None,
                        cancel: None,
                    };
                }
                Err(e) => {
                    warn(
                        ctx,
                        format!(
                            "квитанция {}: {} — повтор на следующем блоке",
                            w.hash, e.message
                        ),
                    )
                    .await;
                    continue;
                }
            }
        }
        since = head.number;
        match cancel::price(ctx, price, &w.cfg, &w.ready) {
            Ok(p) => price = p,
            Err(e) => {
                warn(ctx, format!("отмена {}: {}", w.hash, e.message)).await;
                continue;
            }
        }
        match cancel::send(
            ctx,
            &w.stats,
            &w.ready,
            w.from,
            w.nonce,
            price,
            head.number,
            &w.cfg,
        )
        .await
        {
            Ok(h) => {
                ours.push(h);
                let msg = format!(
                    "{} не попала за send.max_wait_blocks — отмена {h} (nonce {}, {price} wei)",
                    w.hash, w.nonce
                );
                ctx.log(Level::Warn, "send.tx_not_included", msg, None)
                    .await;
            }
            Err(e) => {
                let msg = format!(
                    "отмена {} (nonce {}, {price} wei) не ушла: {} — следующая через send.max_wait_blocks",
                    w.hash, w.nonce, e.message
                );
                warn(ctx, msg).await;
            }
        }
    }
}

/// Итог по найденной квитанции; счётчики канала («в блоке» — только если канал один, см. send/README.md).
fn landed(w: &Watch, ours: &[B256], i: usize, r: Receipt) -> Tracked {
    if i > 0 {
        return Tracked {
            outcome: TxOutcome::NotIncluded,
            receipt: None,
            cancel: Some((ours[i], r)),
        };
    }
    let ok = r.status == U64::from(1u8);
    if let [id] = w.channels.as_slice() {
        let gas = r.gas_bnb();
        w.stats.update(id, |s| {
            s.included += 1;
            if !ok {
                s.reverted += 1;
                if let Some(g) = gas {
                    s.gas_wasted_bnb += g;
                }
            }
        });
    }
    Tracked {
        outcome: if ok {
            TxOutcome::Included
        } else {
            TxOutcome::Reverted
        },
        receipt: Some(r),
        cancel: None,
    }
}

/// Предупреждение трекера в журнал. Ошибок нет.
async fn warn(ctx: &Ctx, msg: String) {
    ctx.log(Level::Warn, "send.track", msg, None).await;
}

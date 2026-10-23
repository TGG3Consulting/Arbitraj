//! Файл: venues/bsc_dex/send/track_util.rs
//! Что делает: части трекера транзакции (send/track.rs): что отслеживать (Watch), итог (Tracked), итог по найденной
//! квитанции со счётчиками канала, журнал без спама — одна запись на смену состояния (сбой узла, нет блоков,
//! отмена не ушла), причина последнего сбоя — для итога «исход неизвестен».
//! Кто вызывает: send/track.rs, exec/txs.rs.
//! От чего зависит: send/receipt.rs, send/stats.rs, send/ready.rs, send/tx.rs (SendCfg), node/heads.rs, ctx.rs.
//! Настройки: нет своих.
//!
//! «В блоке» засчитывается каналу, только если отправляли в один канал (ответ техлида в Issue #7, send/README.md).

use alloy_primitives::{Address, B256, U64, U256};

use crate::core::types::TxOutcome;
use crate::storage::journal::Level;

use super::super::ctx::Ctx;
use super::super::node::heads::Heads;
use super::ready::Ready;
use super::receipt::Receipt;
use super::stats::Stats;
use super::tx::SendCfg;

/// Итог ожидания.
pub struct Tracked {
    pub outcome: TxOutcome,
    /// Квитанция исходной транзакции (в блоке / откат).
    pub receipt: Option<Receipt>,
    /// Отмена, занявшая nonce: хеш и квитанция (исходная не попала).
    pub cancel: Option<(B256, Receipt)>,
    /// Исход не установлен (лимиты отмены, время ожидания) — причина; очередь останавливается до ручной проверки.
    pub unresolved: Option<String>,
}

impl Tracked {
    /// Итог без квитанций. Ошибок нет.
    pub fn bare(outcome: TxOutcome, unresolved: Option<String>) -> Self {
        Self {
            outcome,
            receipt: None,
            cancel: None,
            unresolved,
        }
    }
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

/// Итог по найденной квитанции (индекс 0 — исходная, больше — отмена). Ошибок нет.
pub fn landed(w: &Watch, ours: &[B256], i: usize, r: Receipt) -> Tracked {
    if i > 0 {
        return Tracked {
            outcome: TxOutcome::NotIncluded,
            receipt: None,
            cancel: Some((ours[i], r)),
            unresolved: None,
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
        unresolved: None,
    }
}

/// Журнал трекера без спама: запись — только когда меняется вид сбоя (или он прошёл).
pub struct Quiet {
    ctx: Ctx,
    hash: B256,
    last: Option<&'static str>,
    reason: String,
}

impl Quiet {
    /// Новый журнал трекера транзакции. Ошибок нет.
    pub fn new(ctx: &Ctx, hash: B256) -> Self {
        Self {
            ctx: ctx.clone(),
            hash,
            last: None,
            reason: String::new(),
        }
    }

    /// Сбой вида `kind`: в журнал, если вид сменился. Ошибок нет.
    pub async fn warn(&mut self, kind: &'static str, msg: String) {
        self.reason = msg.clone();
        if self.last != Some(kind) {
            self.last = Some(kind);
            let text = format!("{}: {msg}", self.hash);
            self.ctx.log(Level::Warn, "send.track", text, None).await;
        }
    }

    /// Сбой вида `kind` прошёл: в журнал одна запись. Ошибок нет.
    pub async fn ok(&mut self, kind: &'static str) {
        if self.last == Some(kind) {
            self.last = None;
            let text = format!("{}: снова отвечает ({kind})", self.hash);
            self.ctx.log(Level::Info, "send.track", text, None).await;
        }
    }

    /// Причина последнего сбоя (пусто — сбоев не было). Ошибок нет.
    pub fn reason(&self) -> &str {
        &self.reason
    }
}

/// Следующий блок. None — блока нет за feed.stale_after_ms или поток закрыт (в журнал через quiet). Ошибок нет.
pub async fn next_head(
    rx: &mut tokio::sync::watch::Receiver<Option<super::super::node::heads::Head>>,
    cfg: &SendCfg,
    quiet: &mut Quiet,
) -> Option<u64> {
    match tokio::time::timeout(cfg.stale, rx.changed()).await {
        Ok(Ok(())) => {}
        Ok(Err(_)) => {
            quiet
                .warn("heads", "поток новых блоков закрыт".into())
                .await;
            tokio::time::sleep(cfg.stale).await;
            return None;
        }
        Err(_) => {
            quiet
                .warn(
                    "heads",
                    "нет новых блоков дольше feed.stale_after_ms".into(),
                )
                .await;
            return None;
        }
    }
    quiet.ok("heads").await;
    rx.borrow_and_update().as_ref().map(|h| h.number)
}

/// Приёмник блоков. Ошибок нет.
pub fn heads_rx(
    h: &Heads,
) -> tokio::sync::watch::Receiver<Option<super::super::node::heads::Head>> {
    h.watch()
}

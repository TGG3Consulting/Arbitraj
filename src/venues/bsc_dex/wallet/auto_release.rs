//! Файл: venues/bsc_dex/wallet/auto_release.rs
//! Что делает: решение 408, ответ Тиграна 18.11 на вопрос 9б — очередь кошелька, остановленная до ручной проверки
//! (исход удержанной транзакции не установлен, send/settle.rs halt), снимается САМА, если транзакция всё же вошла в блок:
//! на каждом новом блоке (голова node/heads.rs — та, что уже опрашивается, решение 57), ТОЛЬКО пока очередь стоит
//! с этой транзакцией:
//! 1. nonce удержанной транзакции занят в блоке — eth_getTransactionCount(кошелёк, "latest") > nonce;
//! 2. квитанция по её хешу — eth_getTransactionReceipt: есть, transactionHash — наш хеш, from — наш кошелёк, блок
//!    известен.
//!
//! Оба условия — очередь свободна (запомненный nonce забыт — берётся у узла, как при ручном снятии), SQLite, журнал
//! send.queue_released и событие ядру WalletQueue с пометкой «снята автоматически: транзакция 0x… вошла в блок N»
//! (Telegram send.queue_released — core/queue_notify.rs). nonce занят, а квитанции нашей транзакции нет (nonce занят
//! другой транзакцией) — проверка прекращается, очередь стоит, снимает человек (журнал один раз). Очередь сняли или
//! остановка уже другая — задача кончается без запросов. Ошибка узла — следующий блок.
//! Одна задача на транзакцию (по хешу): остановка в трекере и восстановление после запуска не запускают две.
//! Кто вызывает: send/settle.rs (halt), resubscribe_wallets.rs (остановленная очередь после запуска).
//! От чего зависит: wallet/nonce.rs (Nonces::queue_ref, Hold::queue_ref; QueueRef — здесь), wallet/queue_store.rs,
//! node/heads.rs, node/rpc.rs, send/stats.rs, ctx.rs.
//! Настройки: нет (ритм — голова сети, bsc.blocks_poll_ms).
//!
//! eth_getTransactionCount — execution-apis 16caee27 src/eth/state.yaml; eth_getTransactionReceipt (transactionHash,
//! from, blockNumber; null — транзакции нет в блоке) — execution-apis 16caee27 src/eth/transaction.yaml, схема
//! src/schemas/receipt.yaml:
//! https://github.com/ethereum/execution-apis/blob/16caee274b0d4fb7a0316f294029adac88a75a09/src/eth/transaction.yaml

use std::collections::HashSet;
use std::sync::{Arc, LazyLock, Mutex};

use alloy_primitives::{Address, B256, U64};
use serde::Deserialize;
use serde_json::json;

use crate::core::types::{DexAccountId, WalletQueue, WalletQueueState};
use crate::gateway::events::MarketEvent;
use crate::storage::journal::Level;

use super::super::ctx::Ctx;
use super::super::node::heads::Heads;
use super::super::node::rpc;
use super::super::send::stats::Stats;
use super::queue_store;

/// Решение 408 (9б): состояние очереди без замка (не держит очередь) — для самоснятия по квитанции
/// (release_if). Дешёво клонируется.
#[derive(Clone)]
pub struct QueueRef {
    next: Arc<Mutex<Option<u64>>>,
    state: Arc<Mutex<WalletQueueState>>,
}

impl QueueRef {
    /// Ссылка на состояние очереди (wallet/nonce.rs Nonces::queue_ref, Hold::queue_ref). Ошибок нет.
    pub(super) fn new(next: Arc<Mutex<Option<u64>>>, state: Arc<Mutex<WalletQueueState>>) -> Self {
        Self { next, state }
    }

    /// Состояние очереди. Ошибок нет.
    pub fn state(&self) -> WalletQueueState {
        self.state.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }

    /// Снять остановку, только если стоит именно эта транзакция; запомненный nonce забыт (как Nonces::release).
    /// Принимает: хеш. Возвращает: новое состояние или None — остановка другая или очереди уже не остановлена.
    pub fn release_if(&self, hash: B256) -> Option<WalletQueueState> {
        let mut st = self.state.lock().unwrap_or_else(|p| p.into_inner());
        if !matches!(&*st, WalletQueueState::Halted { hash: Some(h), .. } if *h == hash) {
            return None;
        }
        *st = WalletQueueState::Free;
        *self.next.lock().unwrap_or_else(|p| p.into_inner()) = None;
        Some(st.clone())
    }
}

/// Хеши транзакций, по которым уже идёт задача.
static RUNNING: LazyLock<Mutex<HashSet<B256>>> = LazyLock::new(Default::default);

/// Нужные поля квитанции.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Mined {
    transaction_hash: B256,
    from: Address,
    block_number: U64,
}

/// Что проверять: окружение кошелька и удержанная транзакция.
pub struct Watch {
    pub ctx: Ctx,
    pub heads: Heads,
    pub stats: Stats,
    pub queue: QueueRef,
    pub from: Address,
    pub nonce: u64,
    pub hash: B256,
}

/// Что делает: запустить проверку на каждом блоке (если по этому хешу её ещё нет). Ошибок нет.
pub fn spawn(w: Watch) {
    if !RUNNING
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert(w.hash)
    {
        return;
    }
    tokio::spawn(async move {
        let hash = w.hash;
        run(w).await;
        RUNNING
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&hash);
    });
}

/// Остановка всё та же (Halted с этим хешем). Ошибок нет.
fn same(w: &Watch) -> bool {
    matches!(w.queue.state(), WalletQueueState::Halted { hash: Some(h), .. } if h == w.hash)
}

/// Цикл по блокам (см. шапку). Ошибок нет.
async fn run(w: Watch) {
    let mut rx = w.heads.watch();
    while rx.changed().await.is_ok() {
        if !same(&w) {
            return;
        }
        let Ok(n) =
            rpc::call::<U64>(&w.ctx, "eth_getTransactionCount", json!([w.from, "latest"])).await
        else {
            continue;
        };
        if n.to::<u64>() <= w.nonce {
            continue;
        }
        let Ok(r) =
            rpc::call::<Option<Mined>>(&w.ctx, "eth_getTransactionReceipt", json!([w.hash])).await
        else {
            continue;
        };
        match r {
            Some(m) if m.transaction_hash == w.hash && m.from == w.from => {
                return release(&w, m.block_number.to()).await;
            }
            _ => {
                let t = format!(
                    "Очередь кошелька: nonce {} занят, но квитанции транзакции {:#x} нет — nonce занят другой \
                     транзакцией; очередь стоит, снимает человек",
                    w.nonce, w.hash
                );
                return w.ctx.log(Level::Warn, "send.queue_halted", t, None).await;
            }
        }
    }
}

/// Снять очередь: свободна, SQLite, журнал, событие ядру с пометкой. Ошибок нет.
async fn release(w: &Watch, block: u64) {
    let note = format!(
        "очередь кошелька снята автоматически: транзакция {:#x} вошла в блок {block}",
        w.hash
    );
    let Some(state) = w.queue.release_if(w.hash) else {
        return;
    };
    w.ctx
        .log(Level::Warn, "send.queue_released", note.clone(), None)
        .await;
    queue_store::save(&w.ctx, &state).await;
    w.stats.emit(MarketEvent::WalletQueue(WalletQueue {
        venue: w.ctx.venue(),
        account: w.ctx.account.clone().map(DexAccountId),
        state,
        nonce_taken: None,
        note: Some(note),
    }));
}

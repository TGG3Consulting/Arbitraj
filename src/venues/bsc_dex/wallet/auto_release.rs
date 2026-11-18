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
//! (Telegram send.queue_released — core/queue_notify.rs); снятие и запись — под замком очереди с повторной проверкой
//! «стоит эта транзакция» (ревью t193 М6). Квитанция есть, но не наша (hash / from) — проверка прекращается, снимает
//! человек; nonce занят, а квитанции нет — проверка идёт дальше, пока очередь стоит (ревью t193 М4, журнал один раз).
//! Очередь сняли, остановка другая или кошелёк удалён — задача кончается без запросов; ключ закрыт (секреты не
//! разблокированы) — ждёт без запросов к узлу, после разблокировки проверяет (ревью t193 М5). Ошибка узла — следующий
//! блок.
//! Одна задача на транзакцию (по хешу): остановка в трекере и восстановление после запуска не запускают две.
//! Кто вызывает: send/settle.rs (halt), resubscribe_wallets.rs (остановленная очередь после запуска).
//! От чего зависит: wallet/queue_ref.rs (QueueRef), wallet/queue_store.rs, wallet/signer.rs, node/heads.rs, node/rpc.rs,
//! send/stats.rs, ctx.rs, settings/accounts (кошелёк есть).
//! Настройки: нет (ритм — голова сети, bsc.blocks_poll_ms).
//!
//! eth_getTransactionCount — execution-apis 16caee27 src/eth/state.yaml; eth_getTransactionReceipt (transactionHash,
//! from, blockNumber; null — транзакции нет в блоке) — execution-apis 16caee27 src/eth/transaction.yaml, схема
//! src/schemas/receipt.yaml:
//! https://github.com/ethereum/execution-apis/blob/16caee274b0d4fb7a0316f294029adac88a75a09/src/eth/transaction.yaml

use std::collections::HashSet;
use std::sync::{LazyLock, Mutex};

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
use super::queue_ref::QueueRef;
use super::queue_store;
use super::signer;

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
    /// Адрес кошелька; None — ключ ещё не открыт (секреты закрыты): адрес берётся на каждом блоке, пока не откроются
    /// (ревью t193 М5), без запросов к узлу.
    pub from: Option<Address>,
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
    let mut said = false;
    while rx.changed().await.is_ok() {
        if !same(&w) {
            return;
        }
        // Ревью t193 (М5): кошелёк удалён — задача кончается; ключ закрыт — ждём разблокировки секретов.
        let gone = w
            .ctx
            .account
            .as_deref()
            .is_some_and(|id| w.ctx.settings.accounts().wallet(id).is_none());
        if gone {
            return;
        }
        let Some(from) = w.from.or_else(|| signer::address(&w.ctx).ok()) else {
            continue;
        };
        let Ok(n) =
            rpc::call::<U64>(&w.ctx, "eth_getTransactionCount", json!([from, "latest"])).await
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
            Some(m) if m.transaction_hash == w.hash && m.from == from => {
                return release(&w, m.block_number.to()).await;
            }
            // Ревью t193 (М4): квитанция чужая — окончательно: снимает человек.
            Some(m) => {
                let t = format!(
                    "Очередь кошелька: квитанция по хешу {:#x} не наша (transactionHash {:#x}, from {:#x}) — очередь \
                     стоит, снимает человек",
                    w.hash, m.transaction_hash, m.from
                );
                return w.ctx.log(Level::Warn, "send.queue_halted", t, None).await;
            }
            // Ревью t193 (М4): квитанции нет при занятом nonce — проверка идёт дальше, пока очередь стоит с этой
            // транзакцией (журнал — один раз).
            None => {
                if !said {
                    said = true;
                    let t = format!(
                        "Очередь кошелька: nonce {} занят, квитанции транзакции {:#x} пока нет — проверка на каждом \
                         блоке, пока очередь стоит; снять может и человек",
                        w.nonce, w.hash
                    );
                    w.ctx.log(Level::Warn, "send.queue_halted", t, None).await;
                }
            }
        }
    }
}

/// Снять очередь: под замком очереди (ревью t193 М6) — снова проверка «стоит эта транзакция», свободна, SQLite,
/// журнал, событие ядру с пометкой. Ошибок нет.
async fn release(w: &Watch, block: u64) {
    let note = format!(
        "очередь кошелька снята автоматически: транзакция {:#x} вошла в блок {block}",
        w.hash
    );
    let _g = w.queue.locked().await;
    let Some(state) = w.queue.release_if(w.hash) else {
        return;
    };
    queue_store::save(&w.ctx, &state).await;
    w.ctx
        .log(Level::Warn, "send.queue_released", note.clone(), None)
        .await;
    w.stats.emit(MarketEvent::WalletQueue(WalletQueue {
        venue: w.ctx.venue(),
        account: w.ctx.account.clone().map(DexAccountId),
        state,
        nonce_taken: None,
        note: Some(note),
    }));
}

//! Файл: venues/bsc_dex/node/receipts_task.rs
//! Что делает: задача ленты квитанций (решение 68). На каждый новый номер блока (опрос номера, решение 57, node/heads.rs)
//! разбирает блоки по порядку от последнего разобранного, без пропусков: заголовок eth_getBlockByNumber(n, false) →
//! parentHash = хеш последнего разобранного (иначе реорг) → eth_getBlockReceipts(n) (все квитанции — того же хеша) →
//! логи — подпискам по их фильтрам. Запросы — через очередь узла HTTP (node/rpc.rs, pace.rs) классом Check: события
//! не встают за фоном. Реорг: отменённые блоки снимаются с конца окна, пока хеш не совпадёт с узлом, их логи уходят
//! подпискам с removed: true (пулы перечитываются, как при removed у WebSocket); окно кончилось — обрыв.
//! Сбой узла (нет блока, нет связи, 429) — тот же блок снова через bsc.receipts_retry_ms: блоки не пропускаются.
//! Отставание больше bsc.receipts_max_lag_blocks или не задана настройка — обрыв ленты: все подписки закрыты (пулы
//! недостоверны, слушатели подключаются заново), лента начинается с текущего блока.
//! Кто вызывает: node/receipts.rs (Feed::open запускает).
//! От чего зависит: receipts.rs (Inner), receipts_chain.rs, receipts_io.rs (запросы, раздача), heads.rs, ctx.rs.
//! Настройки: bsc.receipts_max_lag_blocks, bsc.receipts_reorg_depth, bsc.receipts_retry_ms [вся система].
//!
//! eth_getBlockReceipts — https://docs.bnbchain.org/bnb-smart-chain/developers/json_rpc/json-rpc-endpoint/ (публичный
//! узел BNB Chain: метод есть, eth_getLogs выключен, лимит 10K запросов за 5 минут); eth_getBlockByNumber — execution-apis
//! src/eth/block.yaml.

use std::sync::{Arc, Weak};
use std::time::Duration;

use crate::storage::journal::Level;

use super::super::ctx::Ctx;
use super::heads::Heads;
use super::receipts::{Inner, SOURCE};
use super::receipts_chain::{Block, Chain, logs_of};
use super::receipts_io::{Fail, cut, deliver, header, receipts};

/// Задача ленты (см. шапку). Кончается, когда лента никому не нужна (последняя копия Feed удалена).
pub(super) async fn run(weak: Weak<Inner>, ctx: Ctx, heads: Heads) {
    let mut w = heads.watch();
    let mut chain = Chain::default();
    let mut said = String::new();
    loop {
        let Some(inner) = weak.upgrade() else { return };
        let target = w.borrow_and_update().as_ref().map(|h| h.number);
        let res = match target {
            Some(t) => step(&inner, &ctx, &mut chain, t).await,
            None => Ok(true),
        };
        let (wait, why) = match res {
            Ok(true) => (false, None),
            Ok(false) => continue,
            Err(Fail::Retry(why)) => (true, Some((Level::Warn, why))),
            Err(Fail::Break(why)) => {
                chain.clear();
                cut(&inner, &why);
                (true, Some((Level::Error, why)))
            }
        };
        if let Some((level, why)) = why
            && why != said
        {
            ctx.log(level, "venue.ws", format!("{SOURCE}: {why}"), None)
                .await;
            said = why;
        }
        drop(inner);
        if wait {
            match ctx.int("bsc.receipts_retry_ms") {
                Ok(ms) => tokio::time::sleep(Duration::from_millis(ms)).await,
                Err(_) => {
                    if w.changed().await.is_err() {
                        return;
                    }
                }
            }
            continue;
        }
        if w.changed().await.is_err() {
            if let Some(inner) = weak.upgrade() {
                cut(&inner, "поток номеров блоков остановлен");
            }
            return;
        }
    }
}

/// Один шаг: начало ленты, один следующий блок или откат одного отменённого. Возвращает: true — дошли до `target`.
/// Ошибки: Fail.
async fn step(inner: &Arc<Inner>, ctx: &Ctx, chain: &mut Chain, target: u64) -> Result<bool, Fail> {
    let set = |name: &str| ctx.int(name).map_err(|e| Fail::Break(e.message));
    let (max_lag, depth) = (
        set("bsc.receipts_max_lag_blocks")?,
        set("bsc.receipts_reorg_depth")?,
    );
    let Some(top) = chain.top().cloned() else {
        // Начало: подписки, сделанные после этого, получат всё, что позже `target`.
        let h = header(ctx, target).await?;
        chain.push(
            Block {
                number: target,
                hash: h.hash,
                logs: Vec::new(),
            },
            depth as usize,
        );
        inner.last.send_replace(Some(target));
        return Ok(true);
    };
    if target <= top.number {
        return Ok(true);
    }
    if target - top.number > max_lag {
        return Err(Fail::Break(format!(
            "отставание {} блоков больше bsc.receipts_max_lag_blocks = {max_lag} — пулы недостоверны, лента заново",
            target - top.number
        )));
    }
    let n = top.number + 1;
    let h = header(ctx, n).await?;
    if !chain.continues(h.parent_hash) {
        return rewind(inner, ctx, chain).await.map(|_| false);
    }
    let receipts = receipts(ctx, n, h.hash).await?;
    let logs = logs_of(&receipts);
    let any = deliver(inner, &logs);
    chain.push(
        Block {
            number: n,
            hash: h.hash,
            logs: any,
        },
        depth as usize,
    );
    inner.last.send_replace(Some(n));
    Ok(n == target)
}

/// Реорг: снимает с конца окна блоки, чей хеш у узла другой; их логи — подпискам с removed: true. Ошибки: Retry —
/// узел; Break — окно кончилось (реорг глубже bsc.receipts_reorg_depth).
async fn rewind(inner: &Arc<Inner>, ctx: &Ctx, chain: &mut Chain) -> Result<(), Fail> {
    loop {
        let Some(top) = chain.top().cloned() else {
            return Err(Fail::Break(
                "реорг глубже bsc.receipts_reorg_depth — пулы недостоверны, лента заново".into(),
            ));
        };
        if header(ctx, top.number).await?.hash == top.hash {
            return Ok(());
        }
        if let Some((n, removed)) = chain.pop_removed() {
            deliver(inner, &removed);
            inner.last.send_replace(Some(n - 1));
            let msg = format!("{SOURCE}: реорг — блок {n} отменён, его события пулов отменены");
            ctx.log(Level::Warn, "venue.ws", msg, None).await;
        }
    }
}

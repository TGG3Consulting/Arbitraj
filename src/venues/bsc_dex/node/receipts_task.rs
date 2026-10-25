//! Файл: venues/bsc_dex/node/receipts_task.rs
//! Что делает: задача ленты квитанций (решение 68). На каждый новый номер блока (опрос номера, решение 57, node/heads.rs)
//! разбирает блоки по порядку от последнего разобранного, без пропусков: заголовок eth_getBlockByNumber(n, false) →
//! parentHash = хеш последнего разобранного (иначе реорг) → eth_getBlockReceipts(n) (полные и этого блока,
//! receipts_io.rs) → логи подпискам и номер блока — одним действием под блокировкой ленты (Feed::deliver).
//! Запросы — через очередь узла HTTP классом Check (задаёт Feed::open): события не встают за фоном.
//! Реорг: блок с конца окна снимается, только если его хеш у узла другой по двум запросам заголовка (ложный реорг за
//! балансировщиком — нет); логи снятого — подпискам с removed: true (пулы перечитываются, как при removed у WebSocket);
//! окно кончилось — обрыв. Сбой узла, неполные квитанции, ответ больше предела — тот же блок снова через
//! bsc.receipts_retry_ms. Отставание больше bsc.receipts_max_lag_blocks или не задана настройка — обрыв ленты: все
//! подписки закрыты (пулы недостоверны, слушатели подключаются заново), лента — с текущего блока.
//! bsc.pool_events_source больше не receipts — лента обрывается и останавливается (пачки подключаются через новый
//! источник). Бюджет узла: когда окно заполнилось и при смене bsc.node_http_rps — в журнал, сколько вызовов в секунду
//! берёт лента (2 × блоков в секунду по факту, receipts_budget.rs).
//! Кто вызывает: node/receipts.rs (Feed::open запускает).
//! От чего зависит: receipts.rs (Feed), receipts_chain.rs, receipts_io.rs, receipts_budget.rs, heads.rs, ctx.rs.
//! Настройки: bsc.pool_events_source, bsc.receipts_max_lag_blocks, bsc.receipts_reorg_depth, bsc.receipts_retry_ms.

use std::sync::Weak;
use std::time::Duration;

use crate::storage::journal::Level;

use super::super::ctx::Ctx;
use super::heads::Heads;
use super::receipts::{Feed, Inner, SOURCE};
use super::receipts_budget::Budget;
use super::receipts_chain::{Block, Chain, gone, logs_of};
use super::receipts_io::{Fail, header, receipts};

/// Задача ленты (см. шапку). Кончается, когда лента никому не нужна (последняя копия Feed удалена) или источник сменён.
pub(super) async fn run(weak: Weak<Inner>, ctx: Ctx, heads: Heads) {
    let mut w = heads.watch();
    let mut chain = Chain::default();
    let mut budget = Budget::default();
    let mut said = String::new();
    loop {
        let Some(inner) = weak.upgrade() else { return };
        let feed = Feed { inner };
        if ctx.text("bsc.pool_events_source").ok().as_deref() != Some("receipts") {
            // Сначала «задача кончилась», потом обрыв: «соединение», открытое после обрыва, запустит ленту заново.
            feed.stopped();
            feed.cut("источник событий пулов сменён (bsc.pool_events_source) — лента остановлена");
            return;
        }
        let target = w.borrow_and_update().as_ref().map(|h| h.number);
        let res = match target {
            Some(t) => step(&feed, &ctx, &mut chain, t).await,
            None => Ok(true),
        };
        let why = match res {
            // Блоков в секунду меряем только догнав голову: во время догона блоки идут быстрее сети.
            Ok(true) => {
                budget.check(&ctx, &chain).await;
                None
            }
            Ok(false) => continue,
            Err(Fail::Retry(why)) => Some((Level::Warn, why)),
            Err(Fail::Break(why)) => {
                chain.clear();
                feed.cut(&why);
                Some((Level::Error, why))
            }
        };
        if let Some((level, why)) = &why
            && *why != said
        {
            ctx.log(*level, "venue.ws", format!("{SOURCE}: {why}"), None)
                .await;
            said = why.clone();
        }
        drop(feed);
        if why.is_some() {
            match ctx.int("bsc.receipts_retry_ms") {
                Ok(ms) => tokio::time::sleep(Duration::from_millis(ms)).await,
                Err(e) => {
                    // Без паузы повтора лента не идёт: обрыв с причиной, ждём следующего номера блока.
                    if let Some(inner) = weak.upgrade() {
                        Feed { inner }.cut(&e.message);
                    }
                    chain.clear();
                    if w.changed().await.is_err() {
                        return;
                    }
                }
            }
            continue;
        }
        if w.changed().await.is_err() {
            if let Some(inner) = weak.upgrade() {
                Feed { inner }.cut("поток номеров блоков остановлен");
            }
            return;
        }
    }
}

/// Один шаг: начало ленты, один следующий блок или откат одного отменённого. Возвращает: true — дошли до `target`.
/// Ошибки: Fail.
async fn step(feed: &Feed, ctx: &Ctx, chain: &mut Chain, target: u64) -> Result<bool, Fail> {
    let set = |name: &str| ctx.int(name).map_err(|e| Fail::Break(e.message));
    let (max_lag, depth) = (
        set("bsc.receipts_max_lag_blocks")?,
        set("bsc.receipts_reorg_depth")?,
    );
    let depth = usize::try_from(depth).unwrap_or(usize::MAX);
    let Some(top) = chain.top().cloned() else {
        // Начало: подписки, сделанные после этого, получат всё, что позже `target`.
        let h = header(ctx, target).await?;
        chain.push(
            Block {
                number: target,
                hash: h.hash,
                logs: Vec::new(),
            },
            depth,
        );
        feed.deliver(&[], target);
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
        return rewind(feed, ctx, chain, n).await.map(|_| false);
    }
    let r = receipts(ctx, n, &h).await?;
    let any = feed.deliver(&logs_of(&r), n);
    chain.push(
        Block {
            number: n,
            hash: h.hash,
            logs: any,
        },
        depth,
    );
    Ok(n == target)
}

/// Реорг: снимает последний блок окна, если его хеш у узла другой по двум запросам заголовка; логи — подпискам с
/// removed: true, номер — на блок раньше (одним действием). Хеш подтвердился прежним — родитель блока `next` у узла
/// разошёлся с ним (ответ отставшего узла): повтор. Ошибки: Retry — узел или ложный реорг; Break — окно кончилось
/// (реорг глубже bsc.receipts_reorg_depth).
async fn rewind(feed: &Feed, ctx: &Ctx, chain: &mut Chain, next: u64) -> Result<(), Fail> {
    let Some(top) = chain.top().cloned() else {
        return Err(Fail::Break(
            "реорг глубже bsc.receipts_reorg_depth — пулы недостоверны, лента заново".into(),
        ));
    };
    let first = header(ctx, top.number).await?.hash;
    if first == top.hash {
        return Err(Fail::Retry(format!(
            "родитель блока {next} не совпал с блоком {}, а сам блок у узла прежний — повтор",
            top.number
        )));
    }
    let second = header(ctx, top.number).await?.hash;
    if !gone(top.hash, first, second) {
        return Err(Fail::Retry(format!(
            "хеш блока {} у узла расходится между ответами — повтор",
            top.number
        )));
    }
    if let Some((n, removed)) = chain.pop_removed() {
        feed.deliver(&removed, n - 1);
        let msg = format!("{SOURCE}: реорг — блок {n} отменён, его события пулов отменены");
        ctx.log(Level::Warn, "venue.ws", msg, None).await;
    }
    if chain.top().is_none() {
        return Err(Fail::Break(
            "реорг глубже bsc.receipts_reorg_depth — пулы недостоверны, лента заново".into(),
        ));
    }
    Ok(())
}

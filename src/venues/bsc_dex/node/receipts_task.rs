//! Файл: venues/bsc_dex/node/receipts_task.rs
//! Что делает: задача ленты квитанций (решение 68). На каждый новый номер блока (опрос номера, решение 57, node/heads.rs)
//! разбирает блоки по порядку от последнего разобранного, без пропусков: заголовок eth_getBlockByNumber(n, false) →
//! parentHash = хеш последнего разобранного (иначе реорг) → eth_getBlockReceipts(n) (полные и этого блока,
//! receipts_io.rs; заголовок и квитанции — одним JSON-RPC batch, следующие блоки — параллельно конвейером,
//! решение 69) → логи подпискам и номер блока — одним действием под блокировкой ленты (Feed::deliver).
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
//! Настройки: bsc.pool_events_source, bsc.receipts_max_lag_blocks, bsc.receipts_reorg_depth, bsc.receipts_retry_ms,
//! bsc.receipts_ahead_blocks (окно конвейера, receipts_pipe.rs).

use std::sync::Weak;
use std::time::Duration;

use crate::storage::journal::Level;

use super::super::ctx::Ctx;
use super::heads::Heads;
use super::receipts::{Feed, Inner, SOURCE};
use super::receipts_budget::Budget;
use super::receipts_chain::{Block, Chain, logs_of};
use super::receipts_fetch::fetch_block;
use super::receipts_io::{Fail, Header, header, rewind};
use super::receipts_lag::Lag;
use super::receipts_pipe::Pipe;

/// Задача ленты (см. шапку). Кончается, когда лента никому не нужна (последняя копия Feed удалена) или источник сменён.
pub(super) async fn run(weak: Weak<Inner>, ctx: Ctx, heads: Heads) {
    let mut w = heads.watch();
    let mut chain = Chain::default();
    let mut budget = Budget::default();
    let mut pipe: Pipe<Result<(Header, Vec<serde_json::Value>), Fail>> = Pipe::default();
    let mut said = String::new();
    let mut lag = Lag::default();
    loop {
        let Some(inner) = weak.upgrade() else { return };
        let feed = Feed { inner };
        if ctx.text("bsc.pool_events_source").ok().as_deref() != Some("receipts") {
            // Сначала «задача кончилась», потом обрыв: «соединение», открытое после обрыва, запустит ленту заново.
            feed.stopped();
            feed.cut("источник событий пулов сменён (bsc.pool_events_source) — лента остановлена");
            return;
        }
        budget.start(&ctx).await;
        let target = w.borrow_and_update().as_ref().map(|h| h.number);
        let res = match target {
            Some(t) => step(&feed, &ctx, &mut chain, &mut pipe, &mut lag, t).await,
            None => Ok(true),
        };
        let why = match res {
            // Блоков в секунду меряем только догнав голову: во время догона блоки идут быстрее сети.
            Ok(true) => {
                budget.check(&ctx, &chain).await;
                None
            }
            Ok(false) => continue,
            Err(Fail::Retry(why) | Fail::Missing(why)) => {
                if let Some(top) = chain.top() {
                    lag.failed(top.number + 1, &why);
                }
                Some((Level::Warn, why))
            }
            Err(Fail::Break(why)) => {
                chain.clear();
                pipe.clear();
                lag = Lag::default();
                // Сброс: пулы недостоверны сами (Invalidated) — отметка отставания больше не нужна.
                feed.inner.lag.set(0);
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
                    pipe.clear();
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

/// Один шаг: начало ленты, один следующий блок или откат одного отменённого. Следующие блоки до
/// bsc.receipts_ahead_blocks вперёд уже запрашиваются параллельно (конвейер, решение 69), забирается и раздаётся строго
/// следующий по номеру; не пришёл — он же снова, пришедшие после него ждут. Возвращает: true — дошли до `target`.
/// Ошибки: Fail.
async fn step(
    feed: &Feed,
    ctx: &Ctx,
    chain: &mut Chain,
    pipe: &mut Pipe<Result<(Header, Vec<serde_json::Value>), Fail>>,
    lag: &mut Lag,
    target: u64,
) -> Result<bool, Fail> {
    let set = |name: &str| ctx.int(name).map_err(|e| Fail::Break(e.message));
    let (max_lag, depth) = (
        set("bsc.receipts_max_lag_blocks")?,
        set("bsc.receipts_reorg_depth")?,
    );
    let ahead = set("bsc.receipts_ahead_blocks")?;
    let depth = usize::try_from(depth).unwrap_or(usize::MAX);
    let Some(top) = chain.top().cloned() else {
        // Начало: подписки, сделанные после этого, получат всё, что позже `target`.
        pipe.clear();
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
    // Отстали больше bsc.receipts_max_lag_blocks — не сброс, а догон (решение 72): состояние пулов остаётся,
    // котировка и сделка запрещены отметкой «данные пулов отстают на N блоков», пока лента не догнала.
    let behind = target - top.number;
    lag.mark(ctx, &feed.inner.lag, behind, max_lag).await;
    lag.report(ctx, behind, ahead).await;
    let n = top.number + 1;
    // Блок, на котором конвейер стоит, — гонкой на все узлы (решение 71).
    let hot = lag.hot();
    pipe.fill(n, target, ahead, |k| {
        fetch_block(ctx.clone(), feed.inner.nodes.clone(), k, hot == Some(k))
    });
    let got = pipe
        .take(n)
        .await
        .ok_or_else(|| Fail::Retry(format!("запрос блока {n} отменён")))?;
    let (h, r) = match got {
        Ok(x) => x,
        // Пропуск невосполним: блок позади головы не отдаёт ни один узел (гонка на все) — полный сброс.
        Err(Fail::Missing(why)) if hot == Some(n) && n < target => {
            return Err(Fail::Break(format!(
                "пропуск невосполним: блок {n} не отдаёт ни один узел ({why}), голова уже {target} — пулы недостоверны, лента заново{}",
                lag.stuck_text()
            )));
        }
        Err(Fail::Missing(why)) => return Err(Fail::Retry(why)),
        Err(e) => return Err(e),
    };
    if !chain.continues(h.parent_hash) {
        // Реорг: запрошенное вперёд могло быть от старой цепочки — всё заново после отката.
        pipe.clear();
        return rewind(feed, ctx, chain, n).await.map(|_| false);
    }
    lag.passed(n);
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

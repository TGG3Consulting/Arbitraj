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
use super::receipts_chain::Chain;
use super::receipts_io::{Fail, Header};
use super::receipts_lag::Lag;
use super::receipts_pipe::Pipe;

/// Итог запроса блока в конвейере.
pub(super) type Got = Result<(Header, Vec<serde_json::Value>), Fail>;

/// Задача ленты (см. шапку). Кончается, когда лента никому не нужна (последняя копия Feed удалена) или источник сменён.
pub(super) async fn run(weak: Weak<Inner>, ctx: Ctx, heads: Heads) {
    let mut w = heads.watch();
    let mut chain = Chain::default();
    let mut budget = Budget::default();
    let mut pipe: Pipe<Got> = Pipe::default();
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
            Some(t) => {
                super::receipts_step::step(&feed, &ctx, &mut chain, &mut pipe, &mut lag, t).await
            }
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
                // Стоит дольше bsc.receipts_stuck_reset_s — полный сброс с причиной (решение 72).
                match lag.overdue(&ctx, &feed).await {
                    Some(reset) => Some((
                        Level::Error,
                        restart(&feed, &mut chain, &mut pipe, &mut lag, reset),
                    )),
                    None => Some((Level::Warn, why)),
                }
            }
            Err(Fail::Break(why)) => Some((
                Level::Error,
                restart(&feed, &mut chain, &mut pipe, &mut lag, why),
            )),
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

/// Полный сброс ленты: окно, конвейер и отставание — заново, отметка «отстают» снята (пулы недостоверны сами —
/// Invalidated), все подписки закрыты. Возвращает: причину для журнала. Ошибок нет.
fn restart(
    feed: &Feed,
    chain: &mut Chain,
    pipe: &mut Pipe<Got>,
    lag: &mut Lag,
    why: String,
) -> String {
    chain.clear();
    pipe.clear();
    *lag = Lag::default();
    feed.inner.lag.set(0);
    feed.cut(&why);
    why
}

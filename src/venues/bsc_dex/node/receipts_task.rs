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
//! источник). Ошибка в настройке узлов списка (receipts_nodes.rs) — один обрыв с причиной, лента ждёт её исправления.
//! Полный сброс держит запрет котировок («сброс», events_lag.rs), пока пачки старой ленты не сделали пулы
//! недостоверными. Бюджет узла: когда окно заполнилось и при смене bsc.node_http_rps — в журнал, сколько вызовов в секунду
//! берёт лента (2 × блоков в секунду по факту, receipts_budget.rs).
//! Кто вызывает: node/receipts.rs (Feed::open запускает).
//! От чего зависит: receipts.rs (Feed), receipts_chain.rs, receipts_io.rs, receipts_budget.rs, heads.rs, ctx.rs.
//! Настройки: bsc.pool_events_source, bsc.receipts_max_lag_blocks, bsc.receipts_reorg_depth, bsc.receipts_retry_ms,
//! bsc.receipts_ahead_blocks (окно конвейера, receipts_pipe.rs).

use std::sync::Weak;
use std::time::Duration;

use crate::settings::live::Watch;
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
    // О «запрет держится дольше bsc.receipts_stuck_reset_s» уже записано (до снятия запрета).
    let mut hold_told = false;
    // О незаданной bsc.receipts_stuck_reset_s уже записано в этом застревании (решение 77).
    let mut no_reset_told = false;
    loop {
        let Some(inner) = weak.upgrade() else { return };
        let feed = Feed { inner };
        if ctx.text("bsc.pool_events_source").ok().as_deref() != Some("receipts") {
            // Отставание и «сброс» снимаются здесь же (ревью #62): иначе при ws котировок не будет никогда.
            feed.stop_source(
                "источник событий пулов сменён (bsc.pool_events_source) — лента остановлена",
            );
            return;
        }
        // Ошибка в настройке узлов (не https, темп 0, список не читается) — один обрыв с причиной, дальше лента ждёт
        // её исправления: без повторов и сбросов по кругу (ревью #62). Подписка на изменения — до проверки.
        let mut fix = Watch::new(
            ctx.settings.subscribe(),
            vec!["bsc.receipts_node".into(), "bsc.pool_events_source".into()],
        );
        if let Err(e) = super::receipts_nodes::list(&ctx) {
            let why = format!("{} — лента стоит до исправления настройки", e.message);
            if why != said {
                ctx.log(Level::Error, "venue.ws", format!("{SOURCE}: {why}"), None)
                    .await;
                said = why.clone();
            }
            restart(&feed, &mut chain, &mut pipe, &mut lag, why);
            drop(feed);
            if fix.next().await.is_none() {
                return;
            }
            continue;
        }
        drop(fix);
        budget.start(&ctx).await;
        let target = w.borrow_and_update().as_ref().map(|h| h.number);
        let res = match target {
            Some(t) => {
                super::receipts_step::step(&feed, &ctx, &mut chain, &mut pipe, &mut lag, t).await
            }
            None => Ok(true),
        };
        // «Сброс» снимается обычным шагом новой ленты, когда все пачки старой закрыли «соединения» (после Invalidated),
        // а пропуск-пересинхрон (решение 77) — когда все «соединения» перечитали пулы.
        if res.is_ok() && (feed.release_if_settled() || feed.release_if_resynced()) {
            let msg = format!(
                "{SOURCE}: пачки перечитали пулы — запрет котировок снят, торговля снова разрешена"
            );
            ctx.log(Level::Info, "venue.ws", msg, None).await;
        }
        hold_told = hold_note(&ctx, &feed, hold_told).await;
        let missing = matches!(res, Err(Fail::Missing(..)));
        let hard = matches!(res, Err(Fail::Over(..)));
        let why = match res {
            // Блоков в секунду меряем только догнав голову: во время догона блоки идут быстрее сети.
            Ok(true) => {
                budget.check(&ctx, &chain).await;
                None
            }
            Ok(false) => continue,
            Err(Fail::Retry(why) | Fail::Over(why) | Fail::Missing(why, _)) => {
                if let Some(top) = chain.top() {
                    lag.failed(top.number + 1, &why, missing, hard);
                }
                log_hosts(&ctx, &mut lag).await;
                // Восстановление «пропуск + пересинхрон» (решение 77): блок дал bsc.receipts_block_max_tries «жёстких»
                // отказов подряд ИЛИ отставание держится дольше bsc.receipts_stuck_reset_s. Иначе — обычный повтор.
                let k = ctx
                    .int("bsc.receipts_block_max_tries")
                    .ok()
                    .and_then(|v| u32::try_from(v).ok())
                    .unwrap_or(0);
                let skip = match lag.overdue_block(k) {
                    Some(w) => Some(w),
                    None => lag.overdue_behind(&ctx, &feed, &mut no_reset_told).await,
                };
                match skip {
                    Some(reset) => {
                        no_reset_told = false;
                        Some((
                            Level::Warn,
                            skip_to_head(&feed, &mut chain, &mut pipe, &mut lag, reset),
                        ))
                    }
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

/// «Сброс» держится дольше bsc.receipts_stuck_reset_s (старая пачка не закрывает «соединение») — одна запись в
/// журнал: сколько держится и какие пачки держат. Не задана настройка — без записи (о ней пишет застревание блока).
/// Принимает: окружение, ленту, было ли уже записано. Возвращает: записано ли (сбрасывается со снятием). Ошибок нет.
async fn hold_note(ctx: &Ctx, feed: &Feed, told: bool) -> bool {
    let Some(t) = feed.inner.lag.held_for() else {
        return false;
    };
    let Ok(limit) = ctx.int("bsc.receipts_stuck_reset_s") else {
        return told;
    };
    if told || t.as_secs() < limit {
        return told;
    }
    let names = feed.holders().join(", ");
    let msg = format!(
        "{SOURCE}: запрет котировок после сброса держится {} с (дольше bsc.receipts_stuck_reset_s = {limit}) — не закрыты «соединения» пачек старой ленты: {names}",
        t.as_secs()
    );
    ctx.log(Level::Warn, "venue.ws", msg, None).await;
    true
}

/// Пропуск застрявшего участка на текущую голову с пересинхроном (решение 77, вариант B): окно, конвейер и местное
/// отставание — заново (следующий шаг начнёт ленту с головы), подписки НЕ рвутся и поколение НЕ меняется; «сброс»
/// держит запрет котировок, пока пачки не перечитали пулы (Feed::request_resync) — снимает его run. Возвращает:
/// причину для журнала. Ошибок нет.
fn skip_to_head(
    feed: &Feed,
    chain: &mut Chain,
    pipe: &mut Pipe<Got>,
    lag: &mut Lag,
    why: String,
) -> String {
    chain.clear();
    pipe.clear();
    *lag = Lag::default();
    feed.request_resync();
    why
}

/// Один раз на застревание пишет в журнал узлы, на которые уходит повтор блока гонкой (решение 77: hot-swap
/// bsc.receipts_node_urls и 429 видны поимённо, но не на каждый ретрай). Ошибок нет.
async fn log_hosts(ctx: &Ctx, lag: &mut Lag) {
    if lag.hot().is_none() || !lag.take_hosts_told() {
        return;
    }
    let hosts = match super::receipts_nodes::list(ctx) {
        Ok(Some((urls, _))) => urls
            .iter()
            .map(|u| super::rpc_race::host_source(u))
            .collect::<Vec<_>>()
            .join(", "),
        _ => ctx
            .failover
            .urls(ctx, super::failover::Link::Http)
            .map(|v| {
                v.iter()
                    .map(|(_, u)| super::rpc_race::host_source(u))
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_else(|_| "узлы не заданы".into()),
    };
    ctx.log(
        Level::Info,
        "venue.ws",
        format!("{SOURCE}: повтор застрявшего блока — гонка на узлы: {hosts}"),
        None,
    )
    .await;
}

/// Полный сброс ленты: окно, конвейер и отставание — заново, все подписки закрыты; отметка «отстают» НЕ снимается, а
/// держит «сброс» (Feed::reset), пока пачки не сделали пулы недостоверными (Invalidated) — снимает его run. Возвращает: причину для журнала. Ошибок нет.
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
    feed.reset(&why);
    why
}

//! Файл: venues/bsc_dex/pools/listen.rs
//! Что делает: живое состояние пачки пулов: подписка logs → блок B того же узла WebSocket (eth_blockNumber по тому же
//! соединению, после подписки: всё, что позже B, придёт в подписку) → ждём, пока узел HTTP дойдёт до B → чтение
//! состояния на блоке B по HTTP → применение событий после B по порядку (blockNumber, logIndex); пулы — ядру.
//! Блок берётся у узла WS, а не HTTP: если HTTP отстаёт, события между его блоком и подпиской потерялись бы.
//! Реорг (removed: true) или расхождение события с картой — пул перечитывается. Обрыв — пулы пачки недостоверны
//! (событие Invalidated по монетам), пауза venue.ws_reconnect_pause_ms, всё заново.
//! Кто вызывает: pools/market.rs, pools/created.rs (новые пулы).
//! От чего зависит: ctx.rs, node/ws.rs, node/rpc.rs, pools/load.rs, listen_log.rs (один лог), registry.rs, discover.rs.
//! Настройки: venue.ws_reconnect_pause_ms, feed.stale_after_ms [bsc_dex] (сколько ждать узел HTTP); через
//! вызываемые — узел, dex.*.
//!
//! «Removed logs (due to chain reorgs) are pushed with removed: true» —
//! https://github.com/ethereum/execution-apis/blob/16caee274b0d4fb7a0316f294029adac88a75a09/src/eth/subscribe.yaml#L1-L60 ;
//! подписки привязаны к соединению — https://geth.ethereum.org/docs/interacting-with-geth/rpc/pubsub .

use std::time::Duration;

use alloy::rpc::types::Log;
use alloy_primitives::U64;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::gateway::errors::{ErrorCode, GatewayResult};
use crate::gateway::events::MarketEvent;
use crate::storage::journal::Level;

use super::super::check::state::Checks;
use super::super::ctx::Ctx;
use super::super::node::heads::Heads;
use super::super::node::{rpc, ws::Conn};
use super::super::tokens::Tokens;
use super::discover::Found;
use super::mids::Mids;
use super::progress::Signals;
use super::registry::Registry;
use super::{listen_log, load};

/// Окружение слушателей пулов. Дешёво клонируется.
#[derive(Clone)]
pub struct Pools {
    pub ctx: Ctx,
    pub reg: Registry,
    pub tokens: Tokens,
    pub out: mpsc::Sender<MarketEvent>,
    /// Итоги проверок монет и пулов.
    pub checks: Checks,
    /// Последний блок сети.
    pub heads: Heads,
    /// Промежуточные монеты (решение 52).
    pub mids: Mids,
    /// Стартовая загрузка, «пул готов», журнал без спама (pools/progress.rs).
    pub signals: Signals,
}

/// Чем кончилась сессия.
pub enum End {
    /// Ядро закрыло канал событий — слушать больше некому.
    Closed,
    /// Обрыв или ошибка — переподключиться.
    Broken(String),
}

/// Слушает пачку пулов постоянно. Принимает: окружение, имя, найденные пулы с тикерами, фильтр подписки, стартовая ли
/// пачка (её первая загрузка — часть стартовой загрузки, pools/progress.rs).
/// Обрыв после загрузки — пулы недостоверны (Invalidated) до перечитывания; сбой ещё до загрузки (подписка, узел HTTP
/// отстал) — пулы не трогаются (они либо ещё не прочитаны, либо уже недостоверны), повтор после паузы. В журнал — одна
/// запись на вид причины. Ошибок не возвращает; не задана пауза переподключения — остановка с записью в журнал.
pub async fn run(p: Pools, name: String, pools: Vec<(Found, String)>, filter: Value, first: bool) {
    let mut first = first;
    loop {
        let (end, loaded) = session(&p, &name, &pools, &filter).await;
        if std::mem::take(&mut first) {
            p.signals.loaded();
        }
        let why = match end {
            End::Closed => return,
            End::Broken(why) => why,
        };
        let family = pools.first().map_or("", |(f, _)| f.family.title);
        if loaded {
            listen_log::invalidate(&p, &pools);
            let msg = format!("Пулы {family}: {why} — данные недостоверны до перечитывания");
            p.signals.once(&p.ctx, Level::Warn, "venue.ws", msg).await;
        } else {
            let msg =
                format!("Пулы {family}: подписка или первая загрузка не удалась ({why}) — повтор");
            p.signals.once(&p.ctx, Level::Warn, "venue.ws", msg).await;
        }
        match p.ctx.int("venue.ws_reconnect_pause_ms") {
            Ok(ms) => tokio::time::sleep(Duration::from_millis(ms)).await,
            Err(e) => {
                p.ctx
                    .log(
                        Level::Error,
                        "venue.ws",
                        format!("Пулы {name} остановлены: {}", e.message),
                        None,
                    )
                    .await;
                return;
            }
        }
    }
}

/// Одна сессия: подписка, чтение состояния, события до обрыва. Возвращает: конец и была ли загрузка состояния.
async fn session(p: &Pools, name: &str, pools: &[(Found, String)], filter: &Value) -> (End, bool) {
    let start = async {
        let conn = Conn::open(&p.ctx, name).await?;
        let sub = conn.subscribe(json!(["logs", filter])).await?;
        let block: U64 = conn.request("eth_blockNumber", json!([])).await?;
        let block = block.to::<u64>();
        http_reached(p, block).await?;
        GatewayResult::Ok((conn, sub, block))
    };
    let (conn, mut sub, block) = match start.await {
        Ok(x) => x,
        Err(e) => return (End::Broken(e.message), false),
    };
    for (found, ticker) in pools {
        let res = match p.reg.get(&found.reference) {
            Some(e) => load::refresh(&p.ctx, &p.tokens, &e, block).await,
            None => {
                load::entry(
                    &p.ctx,
                    &p.tokens,
                    found,
                    ticker,
                    listen_log::found_usdt(found),
                    block,
                )
                .await
            }
        };
        match res {
            Ok(e) => {
                let pool = e.pool.clone();
                p.reg.put(e);
                if let Err(end) = listen_log::emit(p, pool) {
                    return (end, true);
                }
            }
            Err(e) => {
                let msg = format!(
                    "{}: состояние пула не прочитано: {}",
                    found.family.title, e.message
                );
                p.signals.once(&p.ctx, Level::Error, "dex.pool", msg).await;
            }
        }
    }
    p.signals.pool_ready();
    while let Some(raw) = sub.rx.recv().await {
        let log: Log = match serde_json::from_str(raw.get()) {
            Ok(l) => l,
            Err(e) => return (End::Broken(format!("лог не разобран: {e}")), true),
        };
        if let Err(end) = listen_log::on_log(p, &log).await {
            return (end, true);
        }
    }
    let why = conn
        .end_reason()
        .unwrap_or_else(|| "подписка закрыта".into());
    (End::Broken(why), true)
}

/// Ждёт, пока узел HTTP дойдёт до блока узла WS: проверка на каждый новый блок newHeads, сколько угодно блоков, пока
/// узел HTTP продвигается (отставание провайдера — не сбой). Ошибки: StaleData — блок узла HTTP не менялся дольше
/// feed.stale_after_ms или новых блоков нет; узел; NotConfigured.
async fn http_reached(p: &Pools, block: u64) -> GatewayResult<()> {
    let wait = Duration::from_millis(p.ctx.int("feed.stale_after_ms")?);
    let mut rx = p.heads.watch();
    let (mut last, mut moved) = (0u64, tokio::time::Instant::now());
    loop {
        let http: U64 = rpc::call(&p.ctx, "eth_blockNumber", json!([])).await?;
        let http = http.to::<u64>();
        if http >= block {
            return Ok(());
        }
        if http > last {
            (last, moved) = (http, tokio::time::Instant::now());
        } else if moved.elapsed() > wait {
            return Err(p.ctx.err(
                ErrorCode::StaleData,
                format!("узел HTTP стоит на блоке {http} дольше feed.stale_after_ms, узел WebSocket на {block}"),
            ));
        }
        match tokio::time::timeout(wait, rx.changed()).await {
            Ok(Ok(())) => {}
            Ok(Err(_)) => {
                return Err(p
                    .ctx
                    .err(ErrorCode::Internal, "поток новых блоков остановлен"));
            }
            Err(_) => {
                return Err(p.ctx.err(
                    ErrorCode::StaleData,
                    "нет новых блоков дольше feed.stale_after_ms — узел HTTP не с чем сравнить",
                ));
            }
        }
    }
}

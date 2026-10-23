//! Файл: venues/bsc_dex/pools/listen.rs
//! Что делает: живое состояние пачки пулов: подписка logs → чтение состояния на блоке B (после подписки, чтобы
//! не потерять события) → применение событий с блоком > B по порядку; пулы — ядру событием Pool.
//! Реорг (removed: true) или расхождение события с картой — пул перечитывается. Обрыв — пулы пачки недостоверны
//! (событие Invalidated по монетам), пауза venue.ws_reconnect_pause_ms, всё заново.
//! Кто вызывает: pools/market.rs, pools/created.rs (новые пулы).
//! От чего зависит: ctx.rs, node/ws.rs, node/rpc.rs, pools/load.rs, listen_log.rs (один лог), registry.rs, discover.rs.
//! Настройки: venue.ws_reconnect_pause_ms [bsc_dex]; через вызываемые — узел, dex.*.
//!
//! «Removed logs (due to chain reorgs) are pushed with removed: true» —
//! https://github.com/ethereum/execution-apis/blob/16caee274b0d4fb7a0316f294029adac88a75a09/src/eth/subscribe.yaml#L1-L60 ;
//! подписки привязаны к соединению — https://geth.ethereum.org/docs/interacting-with-geth/rpc/pubsub .

use std::collections::BTreeSet;
use std::time::Duration;

use alloy::rpc::types::Log;
use alloy_primitives::U64;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::gateway::errors::GatewayResult;
use crate::gateway::events::MarketEvent;
use crate::storage::journal::Level;

use super::super::check::state::Checks;
use super::super::ctx::Ctx;
use super::super::node::heads::Heads;
use super::super::node::{rpc, ws::Conn};
use super::super::tokens::Tokens;
use super::discover::Found;
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
}

/// Чем кончилась сессия.
pub enum End {
    /// Ядро закрыло канал событий — слушать больше некому.
    Closed,
    /// Обрыв или ошибка — переподключиться.
    Broken(String),
}

/// Слушает пачку пулов постоянно. Принимает: окружение, имя, найденные пулы с тикерами, фильтр подписки.
/// Ошибок не возвращает: всё — в журнал; не задана пауза переподключения — остановка с записью в журнал.
pub async fn run(p: Pools, name: String, pools: Vec<(Found, String)>, filter: Value) {
    loop {
        let why = match session(&p, &name, &pools, &filter).await {
            End::Closed => return,
            End::Broken(why) => why,
        };
        invalidate(&p, &pools);
        p.ctx
            .log(
                Level::Warn,
                "venue.ws",
                format!("Пулы {name}: {why} — данные недостоверны"),
                None,
            )
            .await;
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

/// Одна сессия: подписка, чтение состояния, события до обрыва.
async fn session(p: &Pools, name: &str, pools: &[(Found, String)], filter: &Value) -> End {
    let start = async {
        let conn = Conn::open(&p.ctx, name).await?;
        let sub = conn.subscribe(json!(["logs", filter])).await?;
        let block: U64 = rpc::call(&p.ctx, "eth_blockNumber", json!([])).await?;
        GatewayResult::Ok((conn, sub, block.to::<u64>()))
    };
    let (conn, mut sub, block) = match start.await {
        Ok(x) => x,
        Err(e) => return End::Broken(e.message),
    };
    for (found, ticker) in pools {
        let res = match p.reg.get(&found.reference) {
            Some(e) => load::refresh(&p.ctx, &p.tokens, &e, block).await,
            None => load::entry(&p.ctx, &p.tokens, found, ticker, found_usdt(found), block).await,
        };
        match res {
            Ok(e) => {
                let pool = e.pool.clone();
                p.reg.put(e);
                if let Err(end) = listen_log::emit(p, pool) {
                    return end;
                }
            }
            Err(e) => {
                let msg = format!(
                    "{} {}: состояние не прочитано: {}",
                    found.family.title, ticker, e.message
                );
                p.ctx.log(Level::Error, "dex.pool", msg, None).await;
            }
        }
    }
    while let Some(raw) = sub.rx.recv().await {
        let log: Log = match serde_json::from_str(raw.get()) {
            Ok(l) => l,
            Err(e) => return End::Broken(format!("лог не разобран: {e}")),
        };
        if let Err(end) = listen_log::on_log(p, &log).await {
            return end;
        }
    }
    End::Broken(
        conn.end_reason()
            .unwrap_or_else(|| "подписка закрыта".into()),
    )
}

/// Пулы пачки недостоверны: отметка в реестре и событие Invalidated по каждой монете.
fn invalidate(p: &Pools, pools: &[(Found, String)]) {
    let mut tickers = BTreeSet::new();
    for (f, t) in pools {
        p.reg.update(&f.reference, |e| e.valid = false);
        tickers.insert(t.clone());
    }
    for t in tickers {
        let _ = p.out.try_send(MarketEvent::Invalidated {
            venue: p.ctx.venue(),
            ticker: t,
        });
    }
}

/// Адрес USDT найденного пула: вторая валюта после монеты.
fn found_usdt(f: &Found) -> alloy_primitives::Address {
    if f.token0 == f.coin {
        f.token1
    } else {
        f.token0
    }
}

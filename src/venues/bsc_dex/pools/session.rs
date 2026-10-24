//! Файл: venues/bsc_dex/pools/session.rs
//! Что делает: одна сессия слушателя пачки пулов (pools/listen.rs): подписка logs → блок B того же узла WebSocket →
//! ждём, пока узел HTTP дойдёт до B → чтение состояния пулов на B → события по порядку до обрыва.
//! Пул, чьё состояние не прочиталось (сбой узла), не теряется до следующего обрыва: он недостоверен (valid = false —
//! причина «данные пула недостоверны» видна в допуске пула), и чтение повторяется на каждом новом блоке (newHeads,
//! состояние — на этом блоке; события до него в нём учтены, после — придут в подписку), не больше
//! dex.pool_load_retries раз. Журнал: одна запись «не прочитан», одна — «прочитан с попытки N» или «так и не прочитан
//! за N попыток». События подписки по пулу без записи в реестре игнорируются (listen_log.rs: реестр не знает пула).
//! Связь с узлом WS — одна запись на площадку при потере и одна при восстановлении (pools/link.rs).
//!
//! Кто вызывает: pools/listen.rs.
//! От чего зависит: node/ws.rs, node/rpc.rs, node/heads.rs, pools/load.rs, listen_log.rs, registry.rs, link.rs.
//! Настройки: feed.stale_after_ms, dex.pool_load_retries [bsc_dex].
//!
//! Подписки привязаны к соединению — https://geth.ethereum.org/docs/interacting-with-geth/rpc/pubsub ;
//! eth_blockNumber — execution-apis src/eth (см. listen.rs).

use std::time::Duration;

use alloy::rpc::types::Log;
use alloy_primitives::U64;
use serde_json::{Value, json};

use crate::gateway::errors::{ErrorCode, GatewayResult};
use crate::storage::journal::Level;

use super::super::node::{rpc, ws::Conn};
use super::discover::Found;
use super::listen::{End, Pools};
use super::{link, listen_log, load};

/// Пул, ждущий повторного чтения: индекс в пачке и сколько повторов уже было.
struct Missing {
    i: usize,
    tries: u64,
}

/// Сессия (см. шапку). Возвращает: конец и была ли загрузка состояния.
pub async fn run(p: &Pools, name: &str, pools: &[(Found, String)], filter: &Value) -> (End, bool) {
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
    link::up(p, name).await;
    let mut missing = Vec::new();
    for (i, (found, ticker)) in pools.iter().enumerate() {
        match load_one(p, found, ticker, block).await {
            Ok(Ok(())) => {}
            Ok(Err(why)) => {
                p.reg.update(&found.reference, |e| e.valid = false);
                let msg = format!(
                    "{} {ticker}: состояние пула не прочитано: {why} — повтор на новых блоках",
                    found.family.title
                );
                p.ctx.log(Level::Warn, "dex.pool", msg, None).await;
                missing.push(Missing { i, tries: 0 });
            }
            Err(end) => return (end, true),
        }
    }
    p.signals.pool_ready();
    let mut heads = p.heads.watch();
    loop {
        tokio::select! {
            raw = sub.rx.recv() => {
                let Some(raw) = raw else { break };
                let log: Log = match serde_json::from_str(raw.get()) {
                    Ok(l) => l,
                    Err(e) => return (End::Broken(format!("лог не разобран: {e}")), true),
                };
                if let Err(end) = listen_log::on_log(p, &log).await {
                    return (end, true);
                }
            }
            changed = heads.changed(), if !missing.is_empty() => {
                if changed.is_err() {
                    return (End::Broken("поток новых блоков остановлен".into()), true);
                }
                let at = heads.borrow_and_update().as_ref().map(|h| h.number);
                if let Some(at) = at
                    && let Err(end) = retry(p, pools, &mut missing, at).await
                {
                    return (end, true);
                }
            }
        }
    }
    let why = conn
        .end_reason()
        .unwrap_or_else(|| "подписка закрыта".into());
    (End::Broken(why), true)
}

/// Повтор чтения ждущих пулов на блоке `at`. Ошибки: End — ядро закрыло канал.
async fn retry(
    p: &Pools,
    pools: &[(Found, String)],
    missing: &mut Vec<Missing>,
    at: u64,
) -> Result<(), End> {
    let limit = match p.ctx.int("dex.pool_load_retries") {
        Ok(n) => n,
        Err(e) => {
            p.signals
                .once(
                    &p.ctx,
                    Level::Error,
                    "dex.pool",
                    format!("Повтор чтения пулов невозможен: {}", e.message),
                )
                .await;
            missing.clear();
            return Ok(());
        }
    };
    let mut keep = Vec::new();
    for mut m in std::mem::take(missing) {
        let (found, ticker) = &pools[m.i];
        m.tries += 1;
        let title = found.family.title;
        match load_one(p, found, ticker, at).await? {
            Ok(()) => {
                let msg = format!(
                    "{title} {ticker}: состояние пула прочитано с попытки {}",
                    m.tries + 1
                );
                p.ctx.log(Level::Info, "dex.pool", msg, None).await;
                p.signals.pool_ready();
            }
            Err(why) if m.tries >= limit => {
                let msg = format!(
                    "{title} {ticker}: состояние пула так и не прочитано за {} попыток: {why} — недостоверен до переподключения",
                    m.tries + 1
                );
                p.ctx.log(Level::Error, "dex.pool", msg, None).await;
            }
            Err(_) => keep.push(m),
        }
    }
    *missing = keep;
    Ok(())
}

/// Чтение одного пула на блоке и отправка ядру. Возвращает: Ok(Ok) — прочитан; Ok(Err(причина)) — нет.
/// Ошибки: End — ядро закрыло канал.
async fn load_one(
    p: &Pools,
    found: &Found,
    ticker: &str,
    block: u64,
) -> Result<Result<(), String>, End> {
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
            listen_log::note_refill(p, &e);
            let pool = e.pool.clone();
            p.reg.put(e);
            listen_log::emit(p, pool).map(Ok)
        }
        Err(e) => Ok(Err(e.message)),
    }
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

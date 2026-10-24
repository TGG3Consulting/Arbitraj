//! Файл: venues/bsc_dex/pools/session.rs
//! Что делает: одна сессия слушателя пачки пулов (pools/listen.rs) на одном соединении WebSocket: приём всех пулов
//! пачки (admit.rs: подписка → блок B того же узла WS → чтение на B → глубокие и мелкие), затем события до обрыва.
//! Пулы, найденные позже (новые пулы сети, история V4), принимаются в идущую сессию тем же соединением — без нового
//! соединения и без перечитывания остальных (pools/attach.rs шлёт их в канал пачки). Подписки создаются только при
//! приёме, пробуждении мелкого пула и после обрыва — не на каждое изменение.
//! Лог глубокого пула — в состояние (listen_log.rs); лог добавления ликвидности мелкого пула — пробуждение (admit.rs).
//! Пул, чьё состояние не прочиталось, перечитывается на новых блоках (опрос блоков node/heads.rs, reload.rs).
//! Связь с узлом WS — одна запись на площадку при потере и одна при восстановлении (pools/link.rs).
//!
//! Кто вызывает: pools/listen.rs.
//! От чего зависит: group.rs, admit.rs, reload.rs, listen_log.rs, events.rs, node/heads.rs, link.rs.
//! Настройки: через вызываемые (feed.stale_after_ms, dex.pool_load_retries, dex.min_pool_liquidity_usd).
//!
//! Подписки привязаны к соединению — https://geth.ethereum.org/docs/interacting-with-geth/rpc/pubsub ;
//! eth_blockNumber — execution-apis src/eth (см. listen.rs).

use std::collections::HashMap;

use alloy::rpc::types::Log;
use tokio::sync::mpsc;

use crate::core::types::PoolRef;

use super::admit::{self, Members};
use super::discover::Found;
use super::events;
use super::group::{Group, Msg};
use super::listen::{End, Pools};
use super::{link, listen_log, reload};

/// Канал пулов, найденных после старта пачки.
pub type Adds = mpsc::UnboundedReceiver<Vec<(Found, String)>>;

/// Сессия (см. шапку). Принимает: окружение, имя, пулы пачки (пополняются принятыми), канал новых пулов.
/// Возвращает: конец и была ли загрузка состояния.
pub async fn run(
    p: &Pools,
    name: &str,
    pools: &mut Vec<(Found, String)>,
    adds: &mut Option<Adds>,
) -> (End, bool) {
    if let Some(rx) = adds.as_mut() {
        while let Ok(more) = rx.try_recv() {
            extend(pools, more);
        }
    }
    let mut g = match Group::open(&p.ctx, name).await {
        Ok(g) => g,
        Err(e) => return (End::Broken(e.message), false),
    };
    let mut m = Members {
        pools: std::mem::take(pools),
        deep: Vec::new(),
        missing: Vec::new(),
        loaded: false,
    };
    let all: Vec<usize> = (0..m.pools.len()).collect();
    let end = match admit::admit(p, &mut g, &mut m, all).await {
        Ok(()) => {
            link::up(p, name).await;
            p.signals.pool_ready();
            listen(p, &mut g, &mut m, adds).await
        }
        Err(end) => (end, m.loaded),
    };
    *pools = m.pools;
    end
}

/// События пачки до обрыва. Возвращает: (конец, true — состояние загружалось).
async fn listen(p: &Pools, g: &mut Group, m: &mut Members, adds: &mut Option<Adds>) -> (End, bool) {
    let mut heads = p.heads.watch();
    let mut idx = index(&m.pools);
    loop {
        tokio::select! {
            msg = g.rx.recv() => {
                let raw = match msg {
                    Some(Msg::Log(raw)) => raw,
                    Some(Msg::Closed) | None => break,
                };
                let log: Log = match serde_json::from_str(raw.get()) {
                    Ok(l) => l,
                    Err(e) => return (End::Broken(format!("лог не разобран: {e}")), true),
                };
                if let Err(end) = on_log(p, g, m, &idx, &log).await {
                    return (end, true);
                }
            }
            more = next_add(adds) => {
                let start = m.pools.len();
                extend(&mut m.pools, more);
                let new: Vec<usize> = (start..m.pools.len()).collect();
                if let Err(end) = admit::admit(p, g, m, new).await {
                    return (end, true);
                }
                idx = index(&m.pools);
            }
            changed = heads.changed(), if !m.missing.is_empty() => {
                if changed.is_err() {
                    return (End::Broken("поток новых блоков остановлен".into()), true);
                }
                let at = heads.borrow_and_update().as_ref().map(|h| h.number);
                if let Some(at) = at
                    && let Err(end) = reload::retry(p, &m.pools, &mut m.missing, at).await
                {
                    return (end, true);
                }
            }
        }
    }
    let why = g
        .conn
        .end_reason()
        .unwrap_or_else(|| "подписка закрыта".into());
    (End::Broken(why), true)
}

/// Один лог: глубокий пул — событие его состояния; мелкий — пробуждение по добавлению ликвидности.
/// Ошибки: End — как у listen_log::on_log и admit::wake.
async fn on_log(
    p: &Pools,
    g: &mut Group,
    m: &mut Members,
    index: &HashMap<PoolRef, usize>,
    log: &Log,
) -> Result<(), End> {
    let single = log.topics().get(1).map(|id| PoolRef::Singleton {
        manager: log.address(),
        id: *id,
    });
    let found = single
        .and_then(|r| index.get(&r))
        .or_else(|| index.get(&PoolRef::Contract(log.address())));
    let Some(&i) = found else {
        return Ok(());
    };
    if m.deep.get(i).copied().unwrap_or(true) {
        // Событие только для мелких (V2 Mint) — у глубокого пула состояние ведёт Sync.
        let family = m.pools[i].0.family;
        if log
            .topic0()
            .is_some_and(|t| events::topics(family).contains(t))
        {
            listen_log::on_log(p, log).await?;
        }
        return Ok(());
    }
    if log.removed {
        return Ok(());
    }
    match log.block_number {
        Some(block) => admit::wake(p, g, m, i, block).await,
        None => Err(End::Broken(
            "лог подписки без blockNumber — блок пробуждения пула неизвестен".into(),
        )),
    }
}

/// Индекс пулов пачки по ссылке. Ошибок нет.
fn index(pools: &[(Found, String)]) -> HashMap<PoolRef, usize> {
    pools
        .iter()
        .enumerate()
        .map(|(i, (f, _))| (f.reference.clone(), i))
        .collect()
}

/// Добавляет пулы, которых в пачке ещё нет. Ошибок нет.
fn extend(pools: &mut Vec<(Found, String)>, more: Vec<(Found, String)>) {
    for x in more {
        if !pools.iter().any(|(f, _)| f.reference == x.0.reference) {
            pools.push(x);
        }
    }
}

/// Следующая пачка новых пулов; канала нет или он закрыт — ждёт вечно. Ошибок нет.
async fn next_add(adds: &mut Option<Adds>) -> Vec<(Found, String)> {
    if let Some(rx) = adds.as_mut()
        && let Some(v) = rx.recv().await
    {
        return v;
    }
    *adds = None;
    std::future::pending().await
}

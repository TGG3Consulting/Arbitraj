//! Файл: venues/bsc_dex/pools/listen_log.rs
//! Что делает: один лог подписки пулов — найти пул (по адресу или по PoolId у менеджера), отбросить уже учтённое
//! (позиция (блок, logIndex) не позже последней учтённой), применить событие, перечитать бины или весь пул (реорг,
//! расхождение), отправить
//! пул ядру. Ошибка чтения — пул недостоверен, событие Invalidated, сессия переподключается; кроме StaleData (у узла
//! уже нет состояния блока события, решение 298, stale_read.rs) — тогда сессия идёт дальше, пул ждёт повтора.
//! Там же: недостоверность пачки пулов (invalidate) и USDT найденного пула (found_usdt) — для listen.rs; пересчёт
//! глубины пула на каждой отправке ядру (pools/depth.rs touch, решение 73).
//! Кто вызывает: pools/session.rs (лог глубокого пула), pools/listen.rs (invalidate).
//! От чего зависит: ctx.rs, node/rpc.rs, events.rs, load.rs, load_bin.rs, load_lb_bins.rs (ревью 290), registry.rs,
//! stale_read.rs (решение 298), refresh_event.rs (перечитывание по событию; решение 313 — замер), lost_events.rs.
//! Решение 313: события пула, не ушедшие ядру из-за полного канала, считаются (emit → lost_events.rs).
//! Решение 314: перечитывания по событию (off_map, Refresh, Bins) — классом dex.event_refresh_urgency (refresh_urgency.rs).
//! Настройки: через вызываемые (log.pool_refresh_timing, log.pool_events_lost_interval_s, dex.event_refresh_urgency).
//!
//! removed: true при реорге — execution-apis subscribe.yaml#L1-L60; порядок применения — (blockNumber, logIndex),
//! как присылает узел (research/flow/bsc_dex.md, раздел 4).

use std::collections::BTreeSet;

use alloy::rpc::types::Log;
use alloy_primitives::U64;
use serde_json::json;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::sync::mpsc::error::TrySendError;

use crate::core::types::PoolRef;
use crate::gateway::errors::GatewayResult;
use crate::gateway::events::MarketEvent;
use crate::storage::journal::Level;

use super::super::node::call::block_tag;
use super::super::node::rpc;
use super::super::quote::emptiness;
use super::discover::Found;
use super::events::{self, Applied};
use super::group::Msg;
use super::listen::{End, Pools};
use super::refresh_event::{self, Event};
use super::refresh_urgency;
use super::registry::Entry;
use super::{load_bin, load_lb_bins};

/// Один лог: найти пул, применить или перечитать, отправить ядру. `waiting` — пул в очереди повтора пачки (решение
/// 298): событие применяется, но перечитывания нет (узел на старом блоке не спрашивается, журнал не пишется) — позиция
/// события, требующего перечитывания, отмечается (stale_read::defer), повтор прочитает пул не раньше неё.
/// Возвращает: true — чтение на блоке события опоздало (StaleData, решение 298): пул недостоверен, Invalidated ушло,
/// вызывающий ставит пул в очередь повтора (stale_read::requeue); false — обработан как раньше.
/// `queued` — канал сессии пачки: сколько сообщений ждёт, пока пул перечитывается (замер решения 313).
/// Ошибки: End — прочая ошибка чтения (пул недостоверен), лог без позиции, ядро закрыло канал.
pub async fn on_log(
    p: &Pools,
    log: &Log,
    waiting: bool,
    queued: &UnboundedReceiver<Msg>,
) -> Result<bool, End> {
    let r = match log.topics().get(1) {
        Some(id)
            if p.reg.contains(&PoolRef::Singleton {
                manager: log.address(),
                id: *id,
            }) =>
        {
            PoolRef::Singleton {
                manager: log.address(),
                id: *id,
            }
        }
        _ => PoolRef::Contract(log.address()),
    };
    let Some(entry) = p.reg.get(&r) else {
        return Ok(false);
    };
    let (Some(block), Some(index)) = (log.block_number, log.log_index) else {
        return Err(End::Broken(
            "лог подписки без blockNumber/logIndex — порядок событий неизвестен".into(),
        ));
    };
    if !log.removed && entry.has_applied(block, index) {
        return Ok(false);
    }
    let applied = if log.removed {
        Applied::Refresh("реорг: событие отменено (removed)".into())
    } else {
        p.reg
            .update(&r, |e| events::apply(e, log))
            .unwrap_or(Applied::Ignored)
    };
    let res = match applied {
        Applied::Ignored => return Ok(false),
        // Решение 298: пул ждёт повтора — не читаем (повтор на новых блоках прочитает его не раньше этого события).
        Applied::Refresh(_) if waiting => {
            if !log.removed {
                super::stale_read::defer(&p.reg, &r, block, index);
            }
            return Ok(false);
        }
        Applied::Updated | Applied::Bins(_) if waiting => Ok(()),
        // Цена ушла так, что полоса глубины вышла за прочитанные слова (решение 73) — карта вокруг нового тика сразу,
        // а не при переподключении: иначе глубина считалась бы не целиком и вердикт мигал бы.
        Applied::Updated => match p.reg.get(&r) {
            Some(e) if super::depth::off_map(&p.ctx, &e) => {
                refresh_event::off_map(p, &e, block, queued)
                    .await
                    .map(|x| p.reg.put(x))
            }
            _ => Ok(()),
        },
        // Решение 314: бины на блоке события — классом dex.event_refresh_urgency (не задана — как раньше). Ревью 314:
        // настройка читается, только если есть что читать у узла (бины есть, ссылка — свой контракт или Singleton;
        // иначе reread_bins к узлу не идёт: пустой список — call::try_many без запроса).
        Applied::Bins(ids)
            if ids.is_empty() || !matches!(r, PoolRef::Contract(_) | PoolRef::Singleton { .. }) =>
        {
            Ok(())
        }
        Applied::Bins(ids) => {
            refresh_urgency::urgent(&p.ctx, reread_bins(p, &r, &ids, block)).await
        }
        Applied::Refresh(why) => {
            p.ctx
                .log(
                    Level::Warn,
                    "dex.pool",
                    format!(
                        "{} {}: {why} — перечитываю",
                        entry.family.title, entry.ticker
                    ),
                    None,
                )
                .await;
            // Реорг: читаем текущий блок; иначе — конец блока события (поздние события блока уже учтены в нём).
            let at = if log.removed {
                refresh_urgency::urgent(&p.ctx, latest(p)).await
            } else {
                Ok(block)
            };
            match at {
                Ok(at) => {
                    refresh_event::refresh(p, &entry, at, &Event::new("refresh", block, queued))
                        .await
                        .map(|e| {
                            p.reg.put(e);
                            p.signals.pool_ready();
                        })
                }
                Err(e) => Err(e),
            }
        }
    };
    if let Err(e) = res {
        p.reg.update(&r, |x| x.valid = false);
        super::invalidated::send(p, entry.coin, entry.ticker.clone());
        // Решение 298: состояния блока события у узла уже нет — сессию пачки не рвём, пул перечитается на новых блоках.
        if super::stale_read::is_stale(&e) {
            super::stale_read::note(p, &entry, block, &e).await;
            return Ok(true);
        }
        return Err(End::Broken(e.message));
    }
    match p.reg.get(&r) {
        Some(e) => {
            note_refill(p, &e);
            emit(p, e.pool).map(|()| false)
        }
        None => Ok(false),
    }
}

/// Пул, исключённый сверкой как пустой, снова не пуст — сигнал сверить его сразу (pools/progress.rs). Ошибок нет.
pub fn note_refill(p: &Pools, e: &Entry) {
    if emptiness::empty(e) {
        return;
    }
    if p.checks
        .pool(&e.pool.reference)
        .quoter
        .any_err(|why| why == emptiness::EMPTY)
    {
        p.signals.refilled(e.pool.reference.clone());
    }
}

/// Перечитывает резервы бинов на блоке события.
async fn reread_bins(p: &Pools, r: &PoolRef, ids: &[u32], block: u64) -> GatewayResult<()> {
    let tag = block_tag(block);
    let uniq: BTreeSet<u32> = ids.iter().copied().collect();
    // Ревью 290: пул Liquidity Book V2.1 (свой контракт) — затронутые бины одной пачкой getBin (load_lb_bins::many).
    if let PoolRef::Contract(pool) = r {
        let list: Vec<u32> = uniq.into_iter().collect();
        for (bin, x, y) in load_lb_bins::many(&p.ctx, *pool, &list, &tag).await? {
            p.reg.update(r, |e| {
                if let Some(b) = e.bins.as_mut() {
                    b.set(bin, x, y);
                }
            });
        }
        return Ok(());
    }
    let PoolRef::Singleton { manager, id } = r else {
        return Ok(());
    };
    for bin in uniq {
        let (x, y) = load_bin::bin(&p.ctx, *manager, *id, bin, &tag).await?;
        p.reg.update(r, |e| {
            if let Some(b) = e.bins.as_mut() {
                b.set(bin, x, y);
            }
        });
    }
    Ok(())
}

/// Текущий номер блока узла (HTTP). Ещё — listen_site.rs (реорг у пар многомонетного пула, решение 295).
pub async fn latest(p: &Pools) -> GatewayResult<u64> {
    let b: U64 = rpc::call(&p.ctx, "eth_blockNumber", json!([])).await?;
    Ok(b.to())
}

/// Пул — ядру без ожидания: медленное ядро не задерживает чтение событий (свежий пул уже в реестре).
pub fn emit(p: &Pools, pool: crate::core::types::Pool) -> Result<(), End> {
    super::depth::touch(p, &pool.reference);
    match p.out.try_send(MarketEvent::Pool(pool)) {
        Ok(()) => Ok(()),
        // Решение 313: канал ядра полон — событие не ушло; счёт и запись не чаще интервала (lost_events.rs).
        Err(TrySendError::Full(_)) => {
            p.signals.lost.note(&p.ctx);
            Ok(())
        }
        Err(TrySendError::Closed(_)) => Err(End::Closed),
    }
}

/// Пулы пачки недостоверны: отметка в реестре и событие Invalidated по каждой монете.
pub fn invalidate(p: &Pools, pools: &[(Found, String)]) {
    // Решение 211: одна монета — один адрес (раньше — набор тикеров).
    let mut coins = std::collections::BTreeMap::new();
    for (f, t) in pools {
        p.reg.update(&f.reference, |e| e.valid = false);
        coins.entry(f.coin).or_insert_with(|| t.clone());
    }
    for (coin, t) in coins {
        super::invalidated::send(p, coin, t);
    }
}

/// Адрес USDT найденного пула: вторая валюта после монеты.
pub fn found_usdt(f: &Found) -> alloy_primitives::Address {
    if f.token0 == f.coin {
        f.token1
    } else {
        f.token0
    }
}

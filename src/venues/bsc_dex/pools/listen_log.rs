//! Файл: venues/bsc_dex/pools/listen_log.rs
//! Что делает: один лог подписки пулов — найти пул (по адресу или по PoolId у менеджера), отбросить уже учтённое
//! (позиция (блок, logIndex) не позже последней учтённой), применить событие, перечитать бины или весь пул (реорг,
//! расхождение), отправить
//! пул ядру. Ошибка чтения — пул недостоверен, событие Invalidated, сессия переподключается.
//! Там же: недостоверность пачки пулов (invalidate) и USDT найденного пула (found_usdt) — для listen.rs; пересчёт
//! глубины пула на каждой отправке ядру (pools/depth.rs touch, решение 73).
//! Кто вызывает: pools/session.rs (лог глубокого пула), pools/listen.rs (invalidate).
//! От чего зависит: ctx.rs, node/rpc.rs, events.rs, load.rs, load_bin.rs, registry.rs.
//! Настройки: через вызываемые.
//!
//! removed: true при реорге — execution-apis subscribe.yaml#L1-L60; порядок применения — (blockNumber, logIndex),
//! как присылает узел (research/flow/bsc_dex.md, раздел 4).

use std::collections::BTreeSet;

use alloy::rpc::types::Log;
use alloy_primitives::U64;
use serde_json::json;
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
use super::listen::{End, Pools};
use super::registry::Entry;
use super::{load, load_bin};

/// Один лог: найти пул, применить или перечитать, отправить ядру.
pub async fn on_log(p: &Pools, log: &Log) -> Result<(), End> {
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
        return Ok(());
    };
    let (Some(block), Some(index)) = (log.block_number, log.log_index) else {
        return Err(End::Broken(
            "лог подписки без blockNumber/logIndex — порядок событий неизвестен".into(),
        ));
    };
    if !log.removed && entry.has_applied(block, index) {
        return Ok(());
    }
    let applied = if log.removed {
        Applied::Refresh("реорг: событие отменено (removed)".into())
    } else {
        p.reg
            .update(&r, |e| events::apply(e, log))
            .unwrap_or(Applied::Ignored)
    };
    let res = match applied {
        Applied::Ignored => return Ok(()),
        // Цена ушла так, что полоса глубины вышла за прочитанные слова (решение 73) — карта вокруг нового тика сразу,
        // а не при переподключении: иначе глубина считалась бы не целиком и вердикт мигал бы.
        Applied::Updated => match p.reg.get(&r) {
            Some(e) if super::depth::off_map(&p.ctx, &e) => {
                let msg = format!(
                    "{} {}: полоса цены dex.depth_band_pct вышла за прочитанные слова битмапа — карта тиков перечитывается вокруг нового тика",
                    e.family.title, e.ticker
                );
                p.ctx.log(Level::Info, "dex.pool", msg, None).await;
                load::refresh(&p.ctx, &p.tokens, &e, block)
                    .await
                    .map(|x| p.reg.put(x))
            }
            _ => Ok(()),
        },
        Applied::Bins(ids) => reread_bins(p, &r, &ids, block).await,
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
                latest(p).await
            } else {
                Ok(block)
            };
            match at {
                Ok(at) => load::refresh(&p.ctx, &p.tokens, &entry, at).await.map(|e| {
                    p.reg.put(e);
                    p.signals.pool_ready();
                }),
                Err(e) => Err(e),
            }
        }
    };
    if let Err(e) = res {
        p.reg.update(&r, |x| x.valid = false);
        super::invalidated::send(p, entry.ticker.clone());
        return Err(End::Broken(e.message));
    }
    match p.reg.get(&r) {
        Some(e) => {
            note_refill(p, &e);
            emit(p, e.pool)
        }
        None => Ok(()),
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
    let PoolRef::Singleton { manager, id } = r else {
        return Ok(());
    };
    let tag = block_tag(block);
    let uniq: BTreeSet<u32> = ids.iter().copied().collect();
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

/// Текущий номер блока узла (HTTP).
async fn latest(p: &Pools) -> GatewayResult<u64> {
    let b: U64 = rpc::call(&p.ctx, "eth_blockNumber", json!([])).await?;
    Ok(b.to())
}

/// Пул — ядру без ожидания: медленное ядро не задерживает чтение событий (свежий пул уже в реестре).
pub fn emit(p: &Pools, pool: crate::core::types::Pool) -> Result<(), End> {
    super::depth::touch(p, &pool.reference);
    match p.out.try_send(MarketEvent::Pool(pool)) {
        Ok(()) | Err(TrySendError::Full(_)) => Ok(()),
        Err(TrySendError::Closed(_)) => Err(End::Closed),
    }
}

/// Пулы пачки недостоверны: отметка в реестре и событие Invalidated по каждой монете.
pub fn invalidate(p: &Pools, pools: &[(Found, String)]) {
    let mut tickers = BTreeSet::new();
    for (f, t) in pools {
        p.reg.update(&f.reference, |e| e.valid = false);
        tickers.insert(t.clone());
    }
    for t in tickers {
        super::invalidated::send(p, t);
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

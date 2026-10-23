//! Файл: venues/bsc_dex/pools/listen_log.rs
//! Что делает: один лог подписки пулов — найти пул (по адресу или по PoolId у менеджера), отбросить уже учтённое
//! (блок ≤ синхронизированного), применить событие, перечитать бины или весь пул (реорг, расхождение), отправить
//! пул ядру. Ошибка чтения — пул недостоверен, событие Invalidated, сессия переподключается.
//! Кто вызывает: pools/listen.rs.
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
use super::events::{self, Applied};
use super::listen::{End, Pools};
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
    let block = log.block_number.unwrap_or(entry.synced_block);
    if !log.removed && block <= entry.synced_block {
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
        Applied::Updated => Ok(()),
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
                latest(p).await.unwrap_or(block)
            } else {
                block
            };
            load::refresh(&p.ctx, &p.tokens, &entry, at)
                .await
                .map(|e| p.reg.put(e))
        }
    };
    if let Err(e) = res {
        p.reg.update(&r, |x| x.valid = false);
        let _ = p.out.try_send(MarketEvent::Invalidated {
            venue: p.ctx.venue(),
            ticker: entry.ticker.clone(),
        });
        return Err(End::Broken(e.message));
    }
    match p.reg.get(&r) {
        Some(e) => emit(p, e.pool),
        None => Ok(()),
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
    match p.out.try_send(MarketEvent::Pool(pool)) {
        Ok(()) | Err(TrySendError::Full(_)) => Ok(()),
        Err(TrySendError::Closed(_)) => Err(End::Closed),
    }
}

//! Файл: venues/bsc_dex/pools/reread_bins.rs
//! Что делает: перечитывание резервов бинов на блоке события (Applied::Bins): Liquidity Book V2.1 — одной пачкой getBin
//! (load_lb_bins::many, ревью 290); Infinity Bin — по одному getBin, решение 325: при dex.event_refresh_batch для DEX
//! пула — все одной пачкой (load_bin::many: node/call.rs view_many — один eth_call к Multicall3 при dex.multicall3, иначе
//! пачка JSON-RPC одним HTTP-запросом), те же ответы и тот же разбор. Вынесено из pools/listen_log.rs (предел размера).
//! Пачке бинов не нужны ни dex.multicall3, ни dex.load_batch (ревью 325, Н6): без Multicall3 view_many — пачка JSON-RPC
//! одним HTTP-запросом, на части не делится.
//! Кто вызывает: pools/listen_log.rs.
//! От чего зависит: load_bin.rs, load_lb_bins.rs, registry.rs, listen.rs (Pools), node/call.rs (block_tag),
//! event_flags.rs (настройка в памяти).
//! Настройки: dex.event_refresh_batch [DEX] (ячейка памяти — event_flags.rs); через вызываемые — dex.multicall3.
//!
//! getBin — ICLPoolManager / IBinPoolManager PancakeSwap Infinity (abi_v4.rs IInfinityBin, research/flow/bsc_v4.md);
//! LBPair.getBin — load_lb_bins.rs.

use std::collections::BTreeSet;

use crate::core::types::PoolRef;
use crate::gateway::errors::GatewayResult;

use super::super::node::call::block_tag;
use super::families::Family;
use super::listen::Pools;
use super::{load_bin, load_lb_bins};

/// Перечитывает резервы бинов на блоке события. Решение 325: у Infinity Bin при dex.event_refresh_batch для DEX пула —
/// все бины одной пачкой (load_bin::many), иначе по одному, как раньше.
pub async fn reread(
    p: &Pools,
    f: &Family,
    r: &PoolRef,
    ids: &[u32],
    block: u64,
) -> GatewayResult<()> {
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
    // Ревью 325 (Н1): настройка — из ячейки памяти (event_flags.rs), не чтением хранилища на каждое событие.
    if super::event_flags::batch(&p.ctx, f) {
        let list: Vec<u32> = uniq.into_iter().collect();
        for (bin, x, y) in load_bin::many(&p.ctx, *manager, *id, &list, &tag).await? {
            p.reg.update(r, |e| {
                if let Some(b) = e.bins.as_mut() {
                    b.set(bin, x, y);
                }
            });
        }
        return Ok(());
    }
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

//! Файл: venues/bsc_dex/pools/per_block_balancer.rs
//! Что делает: ревью 289 — перечитывание всех прочитанных пулов Balancer V2 на новом блоке ОДНОЙ пачкой (Multicall3 /
//! JSON-RPC, частями по dex.load_batch), а не пулом за пулом: вызовы пула — load_balancer::calls (PER_POOL на пул),
//! разбор — load_balancer::decode; запись в реестр и отправка ядру — как reload::load_one. Пул, чьё неизменяемое ещё не
//! в памяти (load_balancer_fixed::cached), — обычным путём reload::load_one. Не прочитался — valid = false.
//! Кто вызывает: pools/per_block.rs (on_head).
//! От чего зависит: load_balancer.rs, load_balancer_fixed.rs, load.rs (renewed, Loaded), load_batch.rs (batch),
//! node/call.rs (try_chunks, block_tag), listen_log.rs, loading (через Pools), reload.rs.
//! Настройки: dex.vault [DEX вида balancer_v2]; dex.load_batch, dex.multicall3 [bsc_dex].

use alloy_primitives::{Address, Bytes};

use crate::core::types::PoolRef;

use super::super::node::call;
use super::discover::Found;
use super::listen::{End, Pools};
use super::load::{self, Loaded};
use super::load_balancer::{self as lb, PER_POOL};
use super::load_balancer_fixed::{Fixed, cached};
use super::{listen_log, load_batch, reload};

/// Пул пачки: запись, неизменяемое, хранилище.
type Item<'a> = (&'a Found, Fixed, Address);

/// Все пулы вида на блоке одной пачкой. Принимает: пулы (с тикерами), номер блока. Ошибки: End — ядро закрыло канал.
pub async fn on_head(p: &Pools, pools: &[&(Found, String)], block: u64) -> Result<(), End> {
    let mut items: Vec<Item> = Vec::new();
    for (f, t) in pools.iter().copied() {
        let PoolRef::Contract(a) = f.reference else {
            continue;
        };
        match (cached(a), p.ctx.dex_addr("dex.vault", f.family.id)) {
            (Some(fx), Ok(v)) => items.push((f, fx, v)),
            _ => {
                if reload::load_one(p, f, t, block).await?.is_err() {
                    p.reg.update(&f.reference, |e| e.valid = false);
                }
            }
        }
    }
    if items.is_empty() {
        return Ok(());
    }
    let calls: Vec<(Address, Bytes)> = items
        .iter()
        .flat_map(|(f, fx, v)| lb::calls(*v, f.reference.address(), fx))
        .collect();
    let size = load_batch::batch(&p.ctx).unwrap_or(calls.len()).max(1);
    let got = call::try_chunks(&p.ctx, &calls, &call::block_tag(block), size).await;
    for ((f, fx, _), part) in items.iter().zip(got.chunks(PER_POOL)) {
        let answers: Result<Vec<_>, String> = part.iter().cloned().collect();
        let Some(e) = p.reg.get(&f.reference) else {
            continue;
        };
        let st = answers.and_then(|a| lb::decode(fx, &a, (e.pool.token0, e.pool.token1)));
        let Ok(state) = st else {
            p.reg.update(&f.reference, |e| e.valid = false);
            continue;
        };
        let loaded = Loaded {
            state,
            ticks: None,
            bins: None,
            fee_pips: None,
        };
        let e = load::renewed(&e, block, loaded);
        listen_log::note_refill(p, &e);
        let pool = e.pool.clone();
        p.reg.put(e);
        p.loading.done(&f.reference);
        p.loading.publish(p).await;
        listen_log::emit(p, pool)?;
    }
    Ok(())
}

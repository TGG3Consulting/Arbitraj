//! Файл: venues/bsc_dex/pools/per_block_lb.rs
//! Что делает: решение 290 — перечитывание состояния всех прочитанных пулов Liquidity Book V2.1 на новом блоке ОДНОЙ
//! пачкой (Multicall3 / JSON-RPC, частями по dex.load_batch): getActiveId, getStaticFeeParameters,
//! getVariableFeeParameters каждого пула (load_lb::calls) и одно время блока на всю пачку. Изменчивая часть комиссии
//! считается от времени блока обмена (PairParameterHelper.updateReferences L412-L421) — меняется без событий пула; резервы
//! бинов меняют только события пула (events_lb.rs перечитывает пул целиком) — бины записи остаются как есть. Пул без
//! состояния LbBin (ещё не прочитан) — обычным путём reload::load_one. Не прочитался — valid = false.
//! Ревью 332 (Б1): в пачке ещё getBin(активный бин памяти); в память пишутся только комиссия и время блока и только
//! при согласии с памятью (lb_head.rs same); активный бин, бины и отметка учтённого не меняются — события блока не
//! отбрасываются. Не согласовано — lb_head::at_synced (сравнение на блоке памяти).
//! Кто вызывает: pools/per_block.rs (on_head).
//! От чего зависит: load_lb.rs, load.rs (renewed, Loaded), load_batch.rs (batch), load_smardex.rs (block_secs),
//! node/call.rs, node/multicall.rs, listen_log.rs, reload.rs, lb_head.rs (ревью 332).
//! Настройки: dex.load_batch, dex.multicall3 [bsc_dex].

use alloy_primitives::{Address, Bytes};

use crate::core::types::PoolState;

use super::super::node::{call, multicall};
use super::discover::Found;
use super::lb_head::{self, PER_POOL};
use super::listen::{End, Pools};
use super::load_lb;
use super::{load_batch, load_smardex, reload};

/// Все пулы вида на блоке одной пачкой. Принимает: пулы (с тикерами), номер блока. Ошибки: End — ядро закрыло канал.
pub async fn on_head(p: &Pools, pools: &[&(Found, String)], block: u64) -> Result<(), End> {
    let mut items: Vec<&Found> = Vec::new();
    for (f, t) in pools.iter().copied() {
        let ready = p
            .reg
            .get(&f.reference)
            .is_some_and(|e| matches!(e.pool.state, PoolState::LbBin(_)) && e.bins.is_some());
        if ready {
            items.push(f);
        } else if reload::load_one(p, f, t, block).await?.is_err() {
            p.reg.update(&f.reference, |e| e.valid = false);
        }
    }
    if items.is_empty() {
        return Ok(());
    }
    let mut calls: Vec<(Address, Bytes)> = items
        .iter()
        .flat_map(|f| {
            let active = p.reg.get(&f.reference).and_then(|e| lb_head::active_of(&e));
            // Ревью 332 (Б1): + getBin(активный бин памяти); ready выше — запись LB есть (иначе бин 0 — «не равны»).
            lb_head::calls(f.reference.address(), active.unwrap_or_default())
        })
        .collect();
    let mc = multicall::address(&p.ctx);
    if let Some(m) = mc {
        calls.push(load_lb::time_call(m));
    }
    let size = load_batch::batch(&p.ctx).unwrap_or(calls.len()).max(1);
    let got = call::try_chunks(&p.ctx, &calls, &call::block_tag(block), size).await;
    let ts = match mc {
        Some(_) => match got.last() {
            Some(Ok(Ok(b))) => load_lb::decode_time(b).ok(),
            _ => None,
        },
        None => load_smardex::block_secs(&p.ctx, block).await.ok(),
    };
    let mut changed: Vec<&Found> = Vec::new();
    for (f, part) in items.iter().zip(got.chunks(PER_POOL)) {
        let Some(e) = p.reg.get(&f.reference) else {
            continue;
        };
        let answers: Result<Vec<Bytes>, String> =
            part.iter().map(|x| x.clone().and_then(|r| r)).collect();
        let st = match (answers, ts, &e.pool.state) {
            (Ok(a), Some(ts), PoolState::LbBin(d)) => lb_head::decode(&a, d, ts).ok(),
            _ => None,
        };
        let Some(d) = st else {
            p.reg.update(&f.reference, |e| e.valid = false);
            continue;
        };
        let (d, bin) = d;
        // Ревью 332 (Б1): не согласовано с памятью — ждут события обмена; память не трогается (lb_head.rs).
        if !lb_head::same(&e, &d, bin) {
            changed.push(*f);
            continue;
        }
        lb_head::put(p, f, lb_head::renewed(&e, block, d)).await?;
    }
    lb_head::at_synced(p, &changed, block).await
}

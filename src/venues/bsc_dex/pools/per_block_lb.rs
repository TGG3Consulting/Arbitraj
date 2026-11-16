//! Файл: venues/bsc_dex/pools/per_block_lb.rs
//! Что делает: решение 290 — перечитывание состояния всех прочитанных пулов Liquidity Book V2.1 на новом блоке ОДНОЙ
//! пачкой (Multicall3 / JSON-RPC, частями по dex.load_batch): getActiveId, getStaticFeeParameters,
//! getVariableFeeParameters каждого пула (load_lb::calls) и одно время блока на всю пачку. Изменчивая часть комиссии
//! считается от времени блока обмена (PairParameterHelper.updateReferences L412-L421) — меняется без событий пула; резервы
//! бинов меняют только события пула (events_lb.rs перечитывает пул целиком) — бины записи остаются как есть. Пул без
//! состояния LbBin (ещё не прочитан) — обычным путём reload::load_one. Не прочитался — valid = false.
//! Кто вызывает: pools/per_block.rs (on_head).
//! От чего зависит: load_lb.rs, load.rs (renewed, Loaded), load_batch.rs (batch), load_smardex.rs (block_secs),
//! node/call.rs, node/multicall.rs, listen_log.rs, reload.rs.
//! Решение 339: не прочитавшийся пул — pool_fail.rs block_failed (причина, пауза повтора при dex.pool_fail_pause_ms).
//! Настройки: dex.load_batch, dex.multicall3 [bsc_dex].

use alloy_primitives::{Address, Bytes};

use crate::core::types::PoolState;

use super::super::node::{call, multicall};
use super::discover::Found;
use super::listen::{End, Pools};
use super::load::{self, Loaded};
use super::load_lb::{self, PER_POOL};
use super::{listen_log, load_batch, load_smardex, reload};

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
        } else if let Err(why) = reload::load_one(p, f, t, block).await? {
            // Решение 339: причина и пауза повтора — pool_fail.rs (не задана dex.pool_fail_pause_ms — только valid).
            super::pool_fail::block_failed(p, f, &why.message).await;
        }
    }
    if items.is_empty() {
        return Ok(());
    }
    let mut calls: Vec<(Address, Bytes)> = items
        .iter()
        .flat_map(|f| load_lb::calls(f.reference.address()))
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
    for (f, part) in items.iter().zip(got.chunks(PER_POOL)) {
        let Some(e) = p.reg.get(&f.reference) else {
            continue;
        };
        let answers: Result<Vec<Bytes>, String> =
            part.iter().map(|x| x.clone().and_then(|r| r)).collect();
        let st = match (answers, ts, &e.pool.state) {
            (Ok(a), Some(ts), PoolState::LbBin(d)) => {
                load_lb::decode(&a, d.x_is_token0, d.bin_step, ts)
            }
            (Err(why), _, _) => Err(why),
            (_, None, _) => Err("время блока не прочитано".to_string()),
            _ => Err("у записи нет состояния LB".to_string()),
        };
        let d = match st {
            Ok(d) => d,
            Err(why) => {
                super::pool_fail::block_failed(p, f, &why).await;
                continue;
            }
        };
        let loaded = Loaded {
            state: PoolState::LbBin(Box::new(d)),
            ticks: None,
            bins: e.bins.clone(),
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

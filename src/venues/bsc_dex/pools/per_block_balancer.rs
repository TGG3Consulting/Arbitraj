//! Файл: venues/bsc_dex/pools/per_block_balancer.rs
//! Что делает: ревью 289 — перечитывание всех прочитанных пулов Balancer V2 на новом блоке ОДНОЙ пачкой (Multicall3 /
//! JSON-RPC, частями по dex.load_batch), а не пулом за пулом: вызовы пула — load_balancer::calls (PER_POOL на пул),
//! разбор — load_balancer::decode; запись в реестр и отправка ядру — как reload::load_one. Пул, чьё неизменяемое ещё не
//! в памяти (load_balancer_fixed::cached), — обычным путём reload::load_one. Не прочитался — valid = false.
//! Решение 295: пары многомонетного пула (PoolRef::Multi) — вызовы один раз на адрес пула, разбор — по паре каждой записи.
//! Кто вызывает: pools/per_block.rs (on_head).
//! От чего зависит: load_balancer.rs, load_balancer_fixed.rs, load.rs (renewed, Loaded), load_batch.rs (batch),
//! node/call.rs (try_chunks, block_tag), loading (через Pools), reload.rs (load_one_block, put_emit, skip_same).
//! Задача t160: при dex.per_block_skip_same неизменившееся состояние пула ядру не шлётся (reload.rs).
//! Решение 339: не прочитавшийся пул — pool_fail.rs block_failed (причина, пауза повтора при dex.pool_fail_pause_ms).
//! Настройки: dex.vault [DEX вида balancer_v2]; dex.load_batch, dex.multicall3 [bsc_dex].

use alloy_primitives::{Address, Bytes};

use crate::core::types::PoolRef;

use super::super::node::call;
use super::discover::Found;
use super::listen::{End, Pools};
use super::load::{self, Loaded};
use super::load_balancer::{self as lb, PER_POOL};
use super::load_balancer_fixed::{Fixed, cached};
use super::{load_batch, reload};

/// Пул пачки: записи (у пула со своим контрактом — одна; у многомонетного пула в режиме dex.multi_pair — все его пары,
/// решение 295), неизменяемое, хранилище, адрес пула.
type Item<'a> = (Vec<&'a Found>, Fixed, Address, Address);

/// Все пулы вида на блоке одной пачкой; вызовы — один раз на адрес пула (у пар одного пула — общие ответы, разбор —
/// по паре каждой записи). Принимает: пулы (с тикерами), номер блока. Ошибки: End — ядро закрыло канал.
pub async fn on_head(p: &Pools, pools: &[&(Found, String)], block: u64) -> Result<(), End> {
    let mut items: Vec<Item> = Vec::new();
    for (f, t) in pools.iter().copied() {
        let Some(a) = f.reference.own_contract() else {
            continue;
        };
        if matches!(f.reference, PoolRef::Multi { .. })
            && let Some(it) = items
                .iter_mut()
                .find(|it| it.3 == a && matches!(it.0[0].reference, PoolRef::Multi { .. }))
        {
            it.0.push(f);
            continue;
        }
        match (
            cached(p.ctx.place.id, a),
            p.ctx.dex_addr("dex.vault", f.family.id),
        ) {
            (Some(fx), Ok(v)) => items.push((vec![f], fx, v, a)),
            _ => {
                if let Err(why) = reload::load_one_block(p, f, t, block).await? {
                    // Решение 339: причина и пауза повтора — pool_fail.rs (не задана — только valid = false).
                    super::pool_fail::block_failed(p, f, &why).await;
                }
            }
        }
    }
    if items.is_empty() {
        return Ok(());
    }
    let calls: Vec<(Address, Bytes)> = items
        .iter()
        .flat_map(|(_, fx, v, a)| lb::calls(*v, *a, fx))
        .collect();
    let size = load_batch::batch(&p.ctx).unwrap_or(calls.len()).max(1);
    let got = call::try_chunks(&p.ctx, &calls, &call::block_tag(block), size).await;
    // Задача t160: неизменившееся состояние ядру не шлётся (reload.rs skip_same, put_emit); не задана — как раньше.
    let skip = reload::skip_same(p);
    for ((list, fx, _, _), part) in items.iter().zip(got.chunks(PER_POOL)) {
        let answers: Result<Vec<_>, String> = part.iter().cloned().collect();
        for f in list {
            let Some(e) = p.reg.get(&f.reference) else {
                continue;
            };
            let st = answers
                .clone()
                .and_then(|a| lb::decode(fx, &a, (e.pool.token0, e.pool.token1)));
            let state = match st {
                Ok(s) => s,
                Err(why) => {
                    super::pool_fail::block_failed(p, f, &why).await;
                    continue;
                }
            };
            let loaded = Loaded {
                state,
                ticks: None,
                bins: None,
                fee_pips: None,
            };
            reload::put_emit(p, load::renewed(&e, block, loaded), skip).await?;
        }
    }
    Ok(())
}

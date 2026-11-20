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
//! Решение 357: пул «в обновлении» (settle.rs pending — логи обмена применены не все) на голове не читается и ядру не
//! уходит (его закрывают логи или settle_close.rs); не прочитался на голове — settle_head.rs head_failed (недостоверен,
//! «нужно полное чтение»): следующие головы читают его ЦЕЛИКОМ (reload::load_one_block), голова достоверность не
//! поднимает никогда (ревью слияния «357 × 339», Б1).
//! Решение 445: и пулы Liquidity Book v2.0 (вид lb_v20) в той же пачке — вызовы на пул по версии (lb_head.rs calls_for,
//! per_pool: v2.0 — getReservesAndId, feeParameters, getBin).
//! Кто вызывает: pools/per_block.rs (on_head).
//! От чего зависит: load_lb.rs, load_batch.rs (batch), load_smardex.rs (block_secs),
//! node/call.rs, node/multicall.rs, listen_log.rs, reload.rs (load_one_block, skip_same), lb_head.rs (ревью 332),
//! settle.rs (pending), settle_head.rs (head_failed, needs_full — решение 357), pool_fail.rs (block_failed — решение 339).
//! Задача t160: при dex.per_block_skip_same неизменившееся состояние пула ядру не шлётся (lb_head::put через
//! reload::put_new — как reload::put_emit у прочих видов).
//! Решение 339: не прочитавшийся пул — pool_fail.rs block_failed (причина, пауза повтора при dex.pool_fail_pause_ms);
//! у неудачной головы — сначала settle_head.rs head_failed (решение 357), затем block_failed.
//! Настройки: dex.load_batch, dex.multicall3 [bsc_dex].

use alloy_primitives::{Address, Bytes};

use crate::core::types::PoolState;

use super::super::node::{call, multicall};
use super::discover::Found;
use super::lb_head;
use super::listen::{End, Pools};
use super::load_lb;
use super::{load_batch, load_smardex, reload};

/// Все пулы вида на блоке одной пачкой. Принимает: пулы (с тикерами), номер блока. Ошибки: End — ядро закрыло канал.
pub async fn on_head(p: &Pools, pools: &[&(Found, String)], block: u64) -> Result<(), End> {
    let mut items: Vec<&Found> = Vec::new();
    for (f, t) in pools.iter().copied() {
        // Решение 357: «в обновлении» — не трогаем (ни полного чтения, ни головы).
        if p.holds.pending(&f.reference) {
            continue;
        }
        let ready = p
            .reg
            .get(&f.reference)
            .is_some_and(|e| matches!(e.pool.state, PoolState::LbBin(_)) && e.bins.is_some());
        // Решение 357 (ревью слияния, Б1): «нужно полное чтение» (голова не прочиталась — settle_head.rs) — пул
        // целиком (состояние и все бины полосы); удачное чтение (registry.rs put) снимает отметку. Голова ему
        // достоверность не вернёт: «равны» не видит неактивных бинов, изменённых пропущенными событиями.
        if ready && !p.holds.needs_full(&f.reference) {
            items.push(f);
        } else if let Err(why) = reload::load_one_block(p, f, t, block).await? {
            // Решение 339: причина и пауза повтора — pool_fail.rs (не задана dex.pool_fail_pause_ms — только valid).
            super::pool_fail::block_failed(p, f, &why).await;
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
            // Решение 445: v2.0 — свои вызовы (lb_head.rs calls_for).
            let v20 = lb_head::is_v20(f);
            lb_head::calls_for(f.reference.address(), active.unwrap_or_default(), v20)
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
    // Задача t160: неизменившееся состояние ядру не шлётся (reload.rs skip_same, put_new); не задана — как раньше.
    let skip = reload::skip_same(p);
    let mut changed: Vec<&Found> = Vec::new();
    // Решение 445: у пулов v2.0 и v2.1 вызовов на пул разное число — части по per_pool каждого.
    let mut at = 0usize;
    for f in items.iter() {
        let n = lb_head::per_pool(lb_head::is_v20(f));
        let part = got.get(at..at + n).unwrap_or(&[]);
        at += n;
        let Some(e) = p.reg.get(&f.reference) else {
            continue;
        };
        let answers: Result<Vec<Bytes>, String> =
            part.iter().map(|x| x.clone().and_then(|r| r)).collect();
        let st = match (answers, ts, &e.pool.state) {
            (Ok(a), Some(ts), PoolState::LbBin(d)) => lb_head::decode(&a, d, ts),
            (Err(why), _, _) => Err(why),
            (_, None, _) => Err("время блока не прочитано".to_string()),
            _ => Err("у записи нет состояния LB".to_string()),
        };
        let (d, bin) = match st {
            Ok(x) => x,
            Err(why) => {
                // Решение 357 (ревью слияния, Б1): недостоверен; вернёт ТОЛЬКО полное чтение (следующие головы).
                p.holds.head_failed(&p.reg, &f.reference);
                // Решение 339: причина и пауза повтора (не задана dex.pool_fail_pause_ms — только valid = false).
                super::pool_fail::block_failed(p, f, &why).await;
                continue;
            }
        };
        // Ревью 332 (Б1): не согласовано с памятью — ждут события обмена; память не трогается (lb_head.rs).
        if !lb_head::same(&e, &d, bin) {
            changed.push(*f);
            continue;
        }
        lb_head::put(p, f, lb_head::renewed(&e, block, d), skip).await?;
    }
    lb_head::at_synced(p, &changed, block, skip).await
}

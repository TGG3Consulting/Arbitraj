//! Файл: venues/bsc_dex/pools/per_block_aero.rs
//! Что делает: решение 470 (T470-07) — лёгкое чтение пулов Aero Slipstream V3 (вариант SlipstreamAero) на новом блоке:
//! ТОЛЬКО комиссия крючка — пачкой по DEX (pools/aero_cl_head.rs: fee(), slot0, конфиг крючка, наблюдения и observe для
//! первой комиссии следующего блока) + MEV-налог модуля крючка (одно чтение на DEX, aero_cl.rs hook_tax). Состояние пула
//! (цена, ликвидность, тики) этим чтением не трогается — его перечитывают события пула (Swap, Mint, Burn… — как у вида
//! slipstream, events.rs) полным чтением (load.rs → aero_cl.rs fee: ещё и проверка крючка пула у фабрики).
//! В память пишется только комиссия (Registry::update на месте — новое поколение, только если комиссия сменилась);
//! сменилась и запись достоверна — пул ядру. Пул «в обновлении» (settle.rs pending) не трогается. Не прочитался —
//! pool_fail.rs block_failed (причина, пауза повтора при dex.pool_fail_pause_ms; valid = false до удачного чтения).
//! Кто вызывает: pools/per_block.rs (on_head).
//! От чего зависит: pools/{aero_cl, aero_cl_head, aero_fee_seen, listen, listen_log, pool_fail, registry}.rs.
//! Настройки: через aero_cl_head.rs — dex.cl_swap_hook, dex.factory, dex.cl_hook_ahead_s [DEX], dex.load_batch.

use alloy_primitives::Address;

use crate::core::types::PoolRef;

use super::super::node::call;
use super::discover::Found;
use super::families::Family;
use super::listen::{End, Pools};
use super::{aero_cl, aero_cl_head, aero_fee_seen, listen_log, pool_fail};

/// Пулы одного DEX на блоке: семейство и пулы с крючком и шагом тиков.
type ByDex<'a> = Vec<(&'static Family, Vec<(&'a Found, Address, i32)>)>;

/// Все пулы вида на блоке: по DEX — одна пачка и один налог. Принимает: пулы (с тикерами), номер блока.
/// Ошибки: End — ядро закрыло канал.
pub async fn on_head(p: &Pools, pools: &[&(Found, String)], block: u64) -> Result<(), End> {
    let mut by: ByDex = Vec::new();
    for (f, _) in pools.iter().copied() {
        let (PoolRef::Contract(a), Some(sp)) = (&f.reference, f.tick_spacing) else {
            continue;
        };
        if p.holds.pending(&f.reference) || p.reg.get(&f.reference).is_none() {
            continue;
        }
        match by.iter_mut().find(|(fam, _)| fam.id == f.family.id) {
            Some((_, list)) => list.push((f, *a, sp)),
            None => by.push((f.family, vec![(f, *a, sp)])),
        }
    }
    let tag = call::block_tag(block);
    for (fam, items) in by {
        if fam.variant.spacing_keyed() || fam.variant.registered_slipstream() {
            pool_fees(p,&items,&tag,block).await?;
            continue;
        }
        let asks: Vec<(Address, i32)> = items.iter().map(|(_, a, sp)| (*a, *sp)).collect();
        let res = match aero_cl::hook_tax(&p.ctx, fam, &tag).await {
            Ok(tax) => aero_cl_head::read(&p.ctx, fam, &asks, block)
                .await
                .map(|list| (tax, list)),
            Err(e) => Err(e),
        };
        let (tax, list) = match res {
            Ok(x) => x,
            Err(e) => {
                for (f, _, _) in &items {
                    pool_fail::block_failed(p, f, &e).await;
                }
                continue;
            }
        };
        for ((f, a, _), r) in items.iter().zip(list) {
            match r {
                // Ревью t240: чтение старого блока не перетирает комиссию нового (aero_fee_seen.rs).
                Ok(view) => {
                    let fee = aero_cl::total(view, tax);
                    put(p, f, aero_fee_seen::newest(p.ctx.place.id, *a, block, fee))?
                }
                Err(why) => pool_fail::block_failed(p, f, &why).await,
            }
        }
    }
    Ok(())
}

/// Администратор может изменить fee() без события. Одна пачка на блок, состояние тиков идёт обычным потоком.
async fn pool_fees(
    p: &Pools,
    items: &[(&Found, Address, i32)],
    tag: &str,
    block: u64,
) -> Result<(), End> {
    let calls = items.iter().map(|(_, address, _)| super::fee_fresh::call(*address)).collect::<Vec<_>>();
    let rows = match call::try_many(&p.ctx, &calls, tag).await {
        Ok(rows) => rows,
        Err(error) => {
            for (found, _, _) in items {
                pool_fail::block_failed(p, found, &error).await;
            }
            return Ok(());
        }
    };
    for (i, (found, address, _)) in items.iter().enumerate() {
        let fee = rows.get(i).and_then(|row| row.as_ref().ok())
            .and_then(|raw| call::decode::<super::abi_check::IPoolImmutables::feeCall>(raw).ok())
            .map(|value| value.to::<u32>()).filter(|fee| *fee < super::super::math::swap_step::MAX_FEE);
        if let Some(fee) = fee {
            put(p, found, aero_fee_seen::newest(p.ctx.place.id, *address, block, fee))?;
        } else {
            let why = p.ctx.err(crate::gateway::errors::ErrorCode::BadResponse,
                "Не получена фактическая комиссия зарегистрированного V3 пула");
            pool_fail::block_failed(p, found, &why).await;
        }
    }
    Ok(())
}

/// Комиссия в запись пула; сменилась и запись достоверна — пул ядру. Ошибки: End — ядро закрыло канал.
fn put(p: &Pools, f: &Found, fee: u32) -> Result<(), End> {
    let changed = p.reg.update(&f.reference, |e| {
        if e.pool.fee_pips == Some(fee) {
            return None;
        }
        e.pool.fee_pips = Some(fee);
        e.valid.then(|| e.pool.clone())
    });
    match changed.flatten() {
        Some(pool) => listen_log::emit(p, pool),
        None => Ok(()),
    }
}

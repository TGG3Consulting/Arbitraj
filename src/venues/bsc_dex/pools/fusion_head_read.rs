//! Файл: venues/bsc_dex/pools/fusion_head_read.rs
//! Что делает: решение 463 (ревью t236, п. 3) — оракул пулов THENA Fusion у края горизонта перечитывается на голове
//! ЗАРАНЕЕ (fusion_head.rs: на этой голове комиссия ещё считается, на следующей начала окна уже не хватит): все такие
//! пулы одновременно (fusion_fee.rs read_fee каждого — globalState, затем две пачки JSON-RPC; getTimepoints идёт от
//! имени пула, поэтому не одним Multicall3), а не по одному перед остальными видами; per_block.rs запускает это вместе
//! с чтениями других видов, обработка блока не ждёт их по очереди.
//! Удачно — данные в книгу (remember в read_fee), комиссия в запись, если пул достоверен и его состояние не новее
//! головы; изменилась — пул ядру. Сбой — причина в журнал dex.pool (Warn); пул остаётся достоверным, пока комиссия
//! считается по прежним данным, на следующей голове без начала окна — недостоверен до полного чтения (fusion_head.rs,
//! settle.rs Need::Fee), а постоянный сбой полного чтения даёт растущую паузу повтора решения 339 (settle_close.rs →
//! pool_fail::note, reload.rs retry); пул на паузе заранее не перечитывается (fusion_head.rs).
//! Кто вызывает: pools/fusion_head.rs (on_head).
//! От чего зависит: fusion_fee.rs (read_fee), listen.rs (Pools, End), registry.rs, emit_pool.rs, depth_book.rs (label),
//! node/call.rs (block_tag).
//! Настройки: через fusion_fee.rs — dex.fusion_fee_horizon_s.

use alloy_primitives::Address;

use crate::core::types::{Pool, PoolState};
use crate::storage::journal::Level;

use super::super::node::call::block_tag;
use super::discover::Found;
use super::listen::{End, Pools};
use super::registry::Entry;

/// Пулы `due` (пул, тикер, адрес) — оракул на голове `block` одновременно (см. шапку). Ошибки: End — ядро закрыло
/// канал.
pub async fn ahead(p: &Pools, due: &[(&Found, &str, Address)], block: u64) -> Result<(), End> {
    if due.is_empty() {
        return Ok(());
    }
    let tag = block_tag(block);
    let reads = due.iter().map(|(f, _, a)| {
        let step = super::synth_farming::step(&p.ctx, f.family);
        let tag = &tag;
        async move { super::fusion_fee::read_fee(&p.ctx, *a, tag, step?).await }
    });
    let got = futures_util::future::join_all(reads).await;
    for ((f, t, _), r) in due.iter().zip(got) {
        match r {
            Ok(fee) => {
                if let Some(Some(pool)) = p.reg.update(&f.reference, |e| put(e, fee, block)) {
                    super::emit_pool::emit(p, pool)?;
                }
            }
            Err(why) => {
                let msg = format!(
                    "{} {t} ({}): оракул пула у края горизонта на голове {block} не перечитан: {} — комиссия \
                     известна до следующей головы, дальше пул недостоверен до полного чтения (решение 463)",
                    f.family.title,
                    super::depth_book::label(&f.reference),
                    why.message
                );
                p.ctx.log(Level::Warn, "dex.pool", msg, None).await;
            }
        }
    }
    Ok(())
}

/// Комиссия чтения на голове `block` — в запись, если пул достоверен и его состояние не новее головы. Возвращает: пул
/// ядру, если комиссия изменилась. Ошибок нет.
fn put(e: &mut Entry, f: u32, block: u64) -> Option<Pool> {
    if !e.valid || e.synced_block > block {
        return None;
    }
    match &mut e.pool.state {
        PoolState::Algebra { fee, .. } if *fee != f => {
            *fee = f;
            Some(e.pool.clone())
        }
        _ => None,
    }
}

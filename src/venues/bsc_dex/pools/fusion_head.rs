//! Файл: venues/bsc_dex/pools/fusion_head.rs
//! Что делает: решение 463 (T463-02, решение координатора 20.11) — комиссия THENA Fusion следующего блока зависит от
//! ВРЕМЕНИ (новая точка оракула: волатильность за время с последней точки, начало окна WINDOW сдвигается), а не только
//! от событий пула. Узел спрашивается только по событиям пула (fusion_fee.rs: чтение пула, Swap); на каждой новой голове
//! комиссия пересчитывается ЛОКАЛЬНО по уже прочитанным данным оракула (remember) на время следующего блока (время
//! головы — node/heads.rs at: один eth_getBlockByNumber на блок на всех, общий с deadline). Изменилась — пул уходит ядру.
//! Данные оракула покрывают начало окна до времени блока чтения + 1 + dex.fusion_fee_horizon_s; голова за этим
//! пределом (пул молчит дольше горизонта): настройка задана — оракул пула перечитывается на голове (fusion_fee.rs
//! read_fee, одно чтение на горизонт), не задана — комиссия держится прежней (как до этого файла).
//! Ещё — комиссия ТОГО ЖЕ блока для сверки с квотером на блоке (решение 73: eth_call квотера на блоке H идёт со временем
//! H — комиссия свопа на H, а не «+ 1 с»): same_block (check/quoter_block.rs).
//! Кто вызывает: pools/fusion_fee.rs (remember), pools/per_block.rs (any, on_head), check/quoter_block.rs (same_block).
//! От чего зависит: math/fee_algebra_v1.rs, fusion_fee.rs, listen.rs (Pools), emit_pool.rs (emit), registry.rs,
//! settle.rs (pending), pool_fail.rs (paused), node/heads.rs, node/call.rs (block_tag).
//! Настройки: dex.fusion_fee_horizon_s [bsc_dex] (через fusion_fee.rs).

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex, MutexGuard};

use alloy_primitives::Address;

use crate::core::types::{Pool, PoolRef, PoolState};

use super::super::math::fee_algebra_v1::{self, Oracle};
use super::super::node::call::block_tag;
use super::discover::Found;
use super::families::Proto;
use super::listen::{End, Pools};
use super::registry::Entry;

/// Данные оракула пула, прочитанные на блоке.
struct Held {
    block: u64,
    ts: u32,
    oracle: Oracle,
}

/// Последние прочитанные данные оракула по адресу пула THENA Fusion.
static BOOK: LazyLock<Mutex<HashMap<Address, Held>>> = LazyLock::new(Default::default);

/// Книга под замком (отравленный замок — данные как есть). Ошибок нет.
fn book() -> MutexGuard<'static, HashMap<Address, Held>> {
    BOOK.lock().unwrap_or_else(|p| p.into_inner())
}

/// Запоминает данные оракула пула, прочитанные на блоке `block` со временем `ts` (более старые не вытесняют более
/// новые). Ошибок нет.
pub fn remember(pool: Address, block: u64, ts: u32, oracle: Oracle) {
    let mut m = book();
    if m.get(&pool).is_some_and(|h| h.block > block) {
        return;
    }
    m.insert(pool, Held { block, ts, oracle });
}

/// Сверка с квотером на блоке (решение 73): в свежем состоянии пула, прочитанном на `block`, комиссия — та, что
/// получит своп в САМОМ этом блоке (fee_at на время блока: точка оракула этого блока уже записана — globalState.fee,
/// иначе пересчёт). Не THENA Fusion или данных этого блока нет — состояние не меняется. Ошибок нет.
pub fn same_block(e: &mut Entry, block: u64) {
    let PoolRef::Contract(addr) = e.pool.reference else {
        return;
    };
    if e.family.proto != Proto::Algebra {
        return;
    }
    let PoolState::Algebra { tick, fee, .. } = &mut e.pool.state else {
        return;
    };
    let m = book();
    let Some(h) = m.get(&addr).filter(|h| h.block == block) else {
        return;
    };
    if let Some(f) = fee_algebra_v1::fee_at(&h.oracle, h.ts, *tick) {
        *fee = f;
    }
}

/// Есть ли в пачке пулы THENA Fusion (голова им нужна). Ошибок нет.
pub fn any(pools: &[(Found, String)]) -> bool {
    pools.iter().any(|(f, _)| f.family.proto == Proto::Algebra)
}

/// Итог шага головы по пулу.
enum Step {
    /// Комиссия изменилась — пул ядру (в коробке: пул крупнее прочих вариантов).
    Changed(Box<Pool>),
    /// Данных оракула на это время нет (за горизонтом или старее записи).
    Read,
    /// Ничего не делать.
    Skip,
}

/// Новая голова `block`: комиссия следующего блока каждого пула THENA Fusion пачки — по данным оракула без узла (см.
/// шапку). Принимает: окружение пачки, пулы, номер головы. Ошибки: End — ядро закрыло канал.
pub async fn on_head(p: &Pools, pools: &[(Found, String)], block: u64) -> Result<(), End> {
    if !any(pools) {
        return Ok(());
    }
    // Время головы: кэш node/heads.rs (один запрос на блок на всех); нет — пересчёта на этой голове нет.
    let Ok((_, ts)) = p.heads.at(&p.ctx).await else {
        return Ok(());
    };
    let ts = ts as u32;
    let reread = p.ctx.int(super::fusion_fee::HORIZON).is_ok();
    for (f, _) in pools {
        if f.family.proto != Proto::Algebra {
            continue;
        }
        let r = &f.reference;
        let PoolRef::Contract(addr) = *r else {
            continue;
        };
        if p.holds.pending(r) || super::pool_fail::paused(p, r) {
            continue;
        }
        match p.reg.update(r, |e| step(e, addr, ts)) {
            Some(Step::Changed(pool)) => super::emit_pool::emit(p, *pool)?,
            Some(Step::Read) if reread => read(p, r, addr, block).await?,
            _ => {}
        }
    }
    Ok(())
}

/// Пересчёт комиссии записи по книге на время головы `ts`. Данные книги — не старее состояния записи
/// (synced_block). Принимает: запись, адрес, время головы. Возвращает: итог шага. Ошибок нет.
fn step(e: &mut Entry, addr: Address, ts: u32) -> Step {
    if !e.valid {
        return Step::Skip;
    }
    let synced = e.synced_block;
    let PoolState::Algebra { tick, fee, .. } = &mut e.pool.state else {
        return Step::Skip;
    };
    let m = book();
    let Some(h) = m.get(&addr).filter(|h| h.block >= synced) else {
        return Step::Read;
    };
    match fee_algebra_v1::next_block_fee(&h.oracle, ts, *tick) {
        Some(n) if n != *fee => {
            *fee = n;
            Step::Changed(Box::new(e.pool.clone()))
        }
        Some(_) => Step::Skip,
        None => Step::Read,
    }
}

/// За горизонтом: оракул пула на голове (fusion_fee.rs read_fee) и комиссия в запись; отметки учтённых событий не
/// меняются (события блока применит сессия). Сбой чтения — комиссия прежняя (пул не трогается). Ошибки: End.
async fn read(p: &Pools, r: &PoolRef, addr: Address, block: u64) -> Result<(), End> {
    let Ok(f) = super::fusion_fee::read_fee(&p.ctx, addr, &block_tag(block)).await else {
        return Ok(());
    };
    let changed = p.reg.update(r, |e| match &mut e.pool.state {
        PoolState::Algebra { fee, .. } if *fee != f => {
            *fee = f;
            Some(e.pool.clone())
        }
        _ => None,
    });
    match changed.flatten() {
        Some(pool) => super::emit_pool::emit(p, pool),
        None => Ok(()),
    }
}

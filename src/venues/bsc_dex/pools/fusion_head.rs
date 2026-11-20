//! Файл: venues/bsc_dex/pools/fusion_head.rs
//! Что делает: решение 463 (T463-02, решение координатора 20.11; ревью t236) — комиссия THENA Fusion следующего блока
//! зависит от ВРЕМЕНИ (новая точка оракула: волатильность за время с последней точки, начало окна WINDOW сдвигается), а
//! не только от событий пула. Узел спрашивается по событиям пула (fusion_fee.rs: чтение пула, Swap); на каждой новой
//! голове комиссия пересчитывается ЛОКАЛЬНО по прочитанным данным оракула (книга — remember) на время следующего блока.
//! Время головы — node/heads.rs time_since: время блока головы или более нового, никогда не более старого (ревью п. 4).
//! Изменилась — пул уходит ядру.
//! Не торгуем вслепую (ревью п. 1): комиссию следующего блока посчитать нельзя (времени головы нет; данных оракула нет
//! или они старее состояния пула; время головы раньше времени чтения или последней точки оракула — без wrapping_sub;
//! начала окна на нужное время нет) — пул недостоверен: отметка settle.rs Need::Fee (valid = false, ядру не уходит),
//! причина — журнал dex.pool. Достоверность возвращает только полное чтение пула (settle_close.rs на следующей голове;
//! не прочитался — причина, пауза и повтор решения 339; первый лог пула — очередь повтора, settle_lost.rs).
//! Край горизонта (ревью п. 3): на следующей голове начала окна уже не хватит (комиссия на время головы + 1 с не
//! считается) — оракул пула перечитывается на этой голове заранее, одновременно у всех таких пулов
//! (fusion_head_read.rs), пока комиссия ещё известна. Пул на паузе решения 339 заранее не перечитывается.
//! Книга (ревью п. 4): снятые пулы — forget (registry.rs retain); лог пула с removed (реорг) — orphan: данные,
//! прочитанные на блоке лога или позже, долой (могли быть с сиротского блока; сравнение номера блока).
//! Сверка с квотером на блоке (решение 73): same_block — комиссия свопа в САМОМ блоке H по данным оракула ЭТОГО ЖЕ
//! чтения (own_read: чтение сверки пишет данные в свою ячейку задачи, а не в книгу).
//! Кто вызывает: pools/fusion_fee.rs (remember), pools/per_block.rs (any, on_head), check/quoter_block.rs (own_read,
//! same_block), registry.rs (forget), listen_log.rs (orphan).
//! От чего зависит: math/fee_algebra_v1.rs, fusion_oracle.rs (Got), fusion_head_read.rs, listen.rs (Pools),
//! emit_pool.rs, registry.rs, settle.rs (pending, hold, Need::Fee), pool_fail.rs (paused), node/heads.rs,
//! depth_book.rs (label).
//! Настройки: через fusion_fee.rs — dex.fusion_fee_horizon_s [bsc_dex].

use std::cell::RefCell;
use std::collections::HashMap;
use std::future::Future;
use std::sync::{LazyLock, Mutex, MutexGuard};

use alloy_primitives::Address;

use crate::core::types::{Pool, PoolRef, PoolState};
use crate::storage::journal::Level;

use super::super::math::fee_algebra_v1;
use super::discover::Found;
use super::families::Proto;
use super::fusion_oracle::Got;
use super::listen::{End, Pools};
use super::registry::Entry;
use super::settle::Need;

/// Данные оракула пула, прочитанные на блоке `block`.
#[derive(Debug, Clone)]
pub struct Held {
    pub block: u64,
    pub got: Got,
}

/// Последние прочитанные данные оракула по адресу пула THENA Fusion.
static BOOK: LazyLock<Mutex<HashMap<Address, Held>>> = LazyLock::new(Default::default);

tokio::task_local! {
    /// Ячейка чтения сверки (own_read): задана — remember пишет сюда, а не в книгу.
    static OWN: RefCell<Option<Held>>;
}

/// Книга под замком (отравленный замок — данные как есть). Ошибок нет.
fn book() -> MutexGuard<'static, HashMap<Address, Held>> {
    BOOK.lock().unwrap_or_else(|p| p.into_inner())
}

/// Запоминает данные оракула пула, прочитанные на блоке `block` (в ячейку own_read, если задача в ней; иначе в книгу:
/// более старый блок не вытесняет более новый, тот же номер — заменяет: после реорга это канонический блок).
/// Ошибок нет.
pub fn remember(pool: Address, block: u64, got: Got) {
    let mut held = Some(Held { block, got });
    if OWN.try_with(|c| *c.borrow_mut() = held.take()).is_ok() {
        return;
    }
    let Some(held) = held else {
        return;
    };
    let mut m = book();
    if m.get(&pool).is_some_and(|h| h.block > block) {
        return;
    }
    m.insert(pool, held);
}

/// Чтение `f` с ячейкой own_read: данные оракула, прочитанные внутри (fusion_fee.rs next_fee), — в итог, не в книгу.
/// Принимает: future чтения. Возвращает: (итог чтения, данные оракула; None — не читались). Ошибок нет.
pub async fn own_read<F: Future>(f: F) -> (F::Output, Option<Held>) {
    OWN.scope(RefCell::new(None), async {
        let out = f.await;
        (out, OWN.with(|c| c.borrow_mut().take()))
    })
    .await
}

/// Сверка с квотером на блоке (решение 73): в свежем состоянии пула, прочитанном на `block`, комиссия — та, что получит
/// своп в САМОМ этом блоке (fee_at на время блока чтения), по данным оракула этого же чтения `own`. Не THENA Fusion —
/// ничего. Принимает: запись, данные чтения, блок. Ошибки (текст): данных чтения нет / не того блока, комиссия не
/// считается.
pub fn same_block(e: &mut Entry, own: Option<&Held>, block: u64) -> Result<(), String> {
    if e.family.proto != Proto::Algebra {
        return Ok(());
    }
    let PoolState::Algebra { tick, fee, .. } = &mut e.pool.state else {
        return Ok(());
    };
    let Some(h) = own.filter(|h| h.block == block) else {
        return Err(format!("THENA Fusion: данных оракула чтения на блоке {block} нет"));
    };
    let o = &h.got.oracle;
    if h.got.ts < o.last.ts {
        return Err(format!("THENA Fusion: время блока {block} раньше последней точки оракула"));
    }
    match fee_algebra_v1::fee_at(o, h.got.ts, *tick) {
        Some(f) => {
            *fee = f;
            Ok(())
        }
        None => Err(format!(
            "THENA Fusion: комиссия блока {block} не считается{}",
            super::fusion_fee::gaps(&h.got.gaps)
        )),
    }
}

/// Пулы `gone` сняты из реестра — их данные долой. Ошибок нет.
pub fn forget(gone: &[PoolRef]) {
    let mut m = book();
    for r in gone {
        if let PoolRef::Contract(a) = r {
            m.remove(a);
        }
    }
}

/// Лог пула с removed (реорг) на блоке `block`: данные, прочитанные на нём или позже, долой. Ошибок нет.
pub fn orphan(r: &PoolRef, block: u64) {
    let PoolRef::Contract(a) = r else {
        return;
    };
    let mut m = book();
    if m.get(a).is_some_and(|h| h.block >= block) {
        m.remove(a);
    }
}

/// Есть ли в пачке пулы THENA Fusion (голова им нужна). Ошибок нет.
pub fn any(pools: &[(Found, String)]) -> bool {
    pools.iter().any(|(f, _)| f.family.proto == Proto::Algebra)
}

/// Итог шага головы по пулу.
enum Step {
    /// Комиссия известна; Some — новая (не равна записи); edge — край горизонта.
    Fee(Option<u32>, bool),
    /// Комиссия не считается — причина.
    Dark(String),
    /// Ничего не делать (недостоверен, не Algebra).
    Skip,
}

/// Новая голова `block`: комиссия следующего блока каждого пула THENA Fusion пачки — по книге без узла (см. шапку).
/// Принимает: окружение пачки, пулы, номер головы. Ошибки: End — ядро закрыло канал.
pub async fn on_head(p: &Pools, pools: &[(Found, String)], block: u64) -> Result<(), End> {
    if !any(pools) {
        return Ok(());
    }
    let ts = match p.heads.time_since(&p.ctx, block).await {
        Ok((_, t)) => u32::try_from(t).map_err(|_| format!("время головы {t} вне uint32")),
        Err(e) => Err(format!("время головы {block} не получено: {}", e.message)),
    };
    let mut due = Vec::new();
    for (f, t) in pools {
        let r = &f.reference;
        let PoolRef::Contract(addr) = *r else {
            continue;
        };
        if f.family.proto != Proto::Algebra || p.holds.pending(r) {
            continue;
        }
        // Чтение записи без изменения реестра (версия реестра растёт только при новой комиссии).
        let Some(e) = p.reg.entry(r) else {
            continue;
        };
        match step(&e, addr, &ts) {
            Step::Fee(new, edge) => {
                if let Some(n) = new
                    && let Some(Some(pool)) = p.reg.update(r, |e| set(e, n))
                {
                    super::emit_pool::emit(p, pool)?;
                }
                if edge && !super::pool_fail::paused(p, r) {
                    due.push((f, t.as_str(), addr));
                }
            }
            Step::Dark(why) => dark(p, f, t, block, &why).await,
            Step::Skip => {}
        }
    }
    super::fusion_head_read::ahead(p, &due, block).await
}

/// Пересчёт комиссии записи по книге на время головы. Данные книги — не старее состояния записи (synced_block).
/// Принимает: запись, адрес, время головы или причина его отсутствия. Возвращает: итог шага. Ошибок нет.
fn step(e: &Entry, addr: Address, ts: &Result<u32, String>) -> Step {
    if !e.valid {
        return Step::Skip;
    }
    let synced = e.synced_block;
    let PoolState::Algebra { tick, fee, .. } = &e.pool.state else {
        return Step::Skip;
    };
    let ts = match ts {
        Ok(t) => *t,
        Err(why) => return Step::Dark(why.clone()),
    };
    let m = book();
    let Some(h) = m.get(&addr) else {
        return Step::Dark("данных оракула нет".into());
    };
    if h.block < synced {
        return Step::Dark(format!(
            "данные оракула (блок {}) старее состояния пула (блок {synced})",
            h.block
        ));
    }
    let (o, read_ts) = (&h.got.oracle, h.got.ts);
    if ts < read_ts || ts < o.last.ts {
        return Step::Dark(format!(
            "время головы {ts} раньше времени чтения {read_ts} или последней точки оракула {}",
            o.last.ts
        ));
    }
    let Some(n) = fee_algebra_v1::next_block_fee(o, ts, *tick) else {
        return Step::Dark(format!(
            "нет начала окна оракула на время {ts} и + 1 с (прочитано на блоке {}, время {read_ts}, горизонт \
             dex.fusion_fee_horizon_s){}",
            h.block,
            super::fusion_fee::gaps(&h.got.gaps)
        ));
    };
    let edge = fee_algebra_v1::next_block_fee(o, ts.saturating_add(1), *tick).is_none();
    Step::Fee((n != *fee).then_some(n), edge)
}

/// Новая комиссия `n` — в запись, если пул достоверен. Возвращает: пул ядру, если комиссия изменилась. Ошибок нет.
fn set(e: &mut Entry, n: u32) -> Option<Pool> {
    if !e.valid {
        return None;
    }
    match &mut e.pool.state {
        PoolState::Algebra { fee, .. } if *fee != n => {
            *fee = n;
            Some(e.pool.clone())
        }
        _ => None,
    }
}

/// Комиссия не считается: пул недостоверен до полного чтения (settle.rs Need::Fee) и причина в журнал. Принимает:
/// окружение, пул, тикер, голова, причина. Ошибок нет.
async fn dark(p: &Pools, f: &Found, t: &str, block: u64, why: &str) {
    p.holds
        .hold(&p.reg, &f.reference, (block, Need::Fee), Vec::new(), false);
    let msg = format!(
        "{} {t} ({}): комиссия следующего блока на голове {block} не считается: {why} — пул недостоверен (в \
         котировки и сделки не идёт, ядру не уходит) до полного чтения на следующей голове (решение 463)",
        f.family.title,
        super::depth_book::label(&f.reference)
    );
    p.ctx.log(Level::Warn, "dex.pool", msg, None).await;
}

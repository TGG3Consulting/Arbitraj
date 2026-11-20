//! Файл: venues/bsc_dex/pools/fusion_head.rs
//! Что делает: решение 463 (T463-02, решение координатора 20.11; ревью t236) — комиссия THENA Fusion следующего блока
//! зависит от ВРЕМЕНИ (новая точка оракула: волатильность за время с последней точки, начало окна WINDOW сдвигается), а
//! не только от событий пула. Узел спрашивается по событиям пула (fusion_fee.rs: чтение пула, Swap); на каждой новой
//! голове комиссия пересчитывается ЛОКАЛЬНО по прочитанным данным оракула (книга — fusion_book.rs) на время следующего
//! блока.
//! Время головы — node/heads.rs time_since: время блока головы или более нового, никогда не более старого (ревью п. 4).
//! Изменилась — пул уходит ядру.
//! Не торгуем вслепую (ревью п. 1): комиссию следующего блока посчитать нельзя (данных оракула нет
//! или они старее состояния пула; время головы раньше времени чтения или последней точки оракула — без wrapping_sub;
//! начала окна на нужное время нет) — пул недостоверен: отметка settle.rs Need::Fee (valid = false, ядру не уходит),
//! причина — журнал dex.pool. Достоверность возвращает только полное чтение пула (settle_close.rs на следующей голове;
//! не прочитался — причина, пауза и повтор решения 339; первый лог пула — очередь повтора, settle_lost.rs).
//! Край горизонта (ревью п. 3): на следующей голове начала окна уже не хватит (комиссия на время головы + 1 с не
//! считается) — оракул пула перечитывается на этой голове заранее, одновременно у всех таких пулов
//! (fusion_head_read.rs), пока комиссия ещё известна. Пул на паузе решения 339 заранее не перечитывается.
//! Книга, её чистка (forget, orphan) и комиссия того же блока для сверки (own_read, same_block) — fusion_book.rs.
//! Повторное ревью t236: времени головы нет — fusion_clock.rs (Need::Clock, без полного чтения; снимается здесь,
//! если книга покрывает); спящие пулы (asleep) пропускаются.
//! Кто вызывает: pools/per_block.rs (any, on_head).
//! От чего зависит: math/fee_algebra_v1.rs, fusion_book.rs (with, Held), fusion_head_read.rs, listen.rs (Pools),
//! emit_pool.rs, registry.rs, settle.rs (pending, hold, Need::Fee), settle_fusion.rs (on_clock, clock_back),
//! fusion_clock.rs (lost), pool_fail.rs (paused), node/heads.rs, depth_book.rs (label).
//! Третье ревью t236 (пп. 2, 3): комиссия и снятие Need::Clock — одним изменением реестра (сначала комиссия, затем
//! достоверность); запись новее той, по которой посчитана комиссия (synced_block), не перетирается.
//! Настройки: через fusion_fee.rs — dex.fusion_fee_horizon_s [bsc_dex].

use alloy_primitives::Address;

use crate::core::types::{Pool, PoolRef, PoolState};
use crate::storage::journal::Level;

use super::super::math::fee_algebra_v1;
use super::discover::Found;
use super::families::Proto;
use super::fusion_book::Held;
use super::listen::{End, Pools};
use super::registry::Entry;
use super::settle::Need;

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
    // Повторное ревью t236 (п. 2): нет времени — пулы недостоверны на этот блок без полного чтения (fusion_clock.rs).
    let ts = match ts {
        Ok(t) => t,
        Err(why) => {
            super::fusion_clock::lost(p, pools, block, &why).await;
            return Ok(());
        }
    };
    let mut due = Vec::new();
    for (f, t) in pools {
        let r = &f.reference;
        let PoolRef::Contract(addr) = *r else {
            continue;
        };
        let clock = p.holds.on_clock(r);
        if f.family.proto != Proto::Algebra || (p.holds.pending(r) && !clock) {
            continue;
        }
        // Чтение записи без изменения реестра (версия реестра растёт только при новой комиссии).
        let Some(e) = p.reg.entry(r) else {
            continue;
        };
        match step(&e, addr, ts, clock) {
            Step::Fee(new, edge) => {
                // Третье ревью t236 (пп. 2, 3): комиссия, затем (Need::Clock: время снова есть, книга покрывает)
                // достоверность до отметки — ОДНИМ изменением реестра и только если запись не новее посчитанной.
                let was = if clock {
                    p.holds.get(r).map(|h| h.was_valid)
                } else {
                    None
                };
                let done = p.reg.update(r, |x| set(x, new, e.synced_block, was));
                if clock && matches!(done, Some(Some(_))) {
                    p.holds.clock_back(r);
                }
                if let Some(Some(Some(pool))) = done {
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
/// Недостоверная запись — только с отметкой Need::Clock (`clock`: недостоверна ею); спящая (asleep: обмены не
/// приходят, не котируется; при пробуждении — полное чтение) — пропуск, её оракул не читается.
/// Принимает: запись, адрес, время головы, стоит ли Need::Clock. Возвращает: итог шага. Ошибок нет.
fn step(e: &Entry, addr: Address, ts: u32, clock: bool) -> Step {
    if (!e.valid && !clock) || e.asleep {
        return Step::Skip;
    }
    let synced = e.synced_block;
    let PoolState::Algebra { tick, fee, .. } = &e.pool.state else {
        return Step::Skip;
    };
    super::fusion_book::with(addr, |h| judge(h, (synced, ts), (*tick, *fee)))
}

/// Комиссия по данным книги `h` на время головы. Принимает: данные книги, (synced_block записи, время головы),
/// (тик, комиссия записи). Возвращает: итог шага. Ошибок нет.
fn judge(h: Option<&Held>, (synced, ts): (u64, u32), (tick, fee): (i32, u32)) -> Step {
    let Some(h) = h else {
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
    let Some(n) = fee_algebra_v1::next_block_fee(o, ts, tick) else {
        return Step::Dark(format!(
            "нет начала окна оракула на время {ts} и + 1 с (прочитано на блоке {}, время {read_ts}, горизонт \
             dex.fusion_fee_horizon_s){}",
            h.block,
            super::fusion_fee::gaps(&h.got.gaps)
        ));
    };
    let edge = fee_algebra_v1::next_block_fee(o, ts.saturating_add(1), tick).is_none();
    Step::Fee((n != fee).then_some(n), edge)
}

/// Комиссия `n` (None — та же) в запись, затем достоверность `was` (Need::Clock снимается) — одним изменением; запись
/// новее посчитанной (`synced` ≠ synced_block) — ничего (третье ревью t236, п. 3). Недостоверная без `was` — ничего.
/// Принимает: запись, комиссию, synced_block расчёта, достоверность до отметки Clock. Возвращает: None — запись новее;
/// Some(пул ядру, если комиссия изменилась и пул достоверен). Ошибок нет.
fn set(e: &mut Entry, n: Option<u32>, synced: u64, was: Option<bool>) -> Option<Option<Pool>> {
    if e.synced_block != synced {
        return None;
    }
    if !e.valid && was.is_none() {
        return Some(None);
    }
    let mut changed = false;
    if let (Some(n), PoolState::Algebra { fee, .. }) = (n, &mut e.pool.state)
        && *fee != n
    {
        *fee = n;
        changed = true;
    }
    if let Some(w) = was {
        e.valid = w;
    }
    Some((changed && e.valid).then(|| e.pool.clone()))
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

//! Файл: venues/bsc_dex/check/quoter_memory_fix.rs
//! Что делает: решение 382 — «память отстаёт от сети» (решение 261: свежее состояние пула на голове H не равно памяти,
//! последнее применённое событие не позже H − dex.quoter_memory_lag_blocks — check/quoter_block.rs memory_lag). Ревью Б1:
//! это ещё не ошибка — событие блока H может быть в пути. Ошибку доказывает только сравнение памяти с узлом на блоке
//! САМОЙ памяти (check/memory_proof.rs); доказана — пул на ПОЛНОЕ чтение своей сессией пачки (pools/settle_reread.rs —
//! до него недостоверен, не котируется), запись Warn. Отставание или «не доказано» — только прежняя заметка
//! (dex.quoter_memory, экран «Глубина»), без отметки. Зацикливания нет: после полного чтения память равна узлу на
//! своём блоке — доказательство её не пометит. Частота — не чаще сверки пула (dex.quoter_check_s).
//! Кто вызывает: check/quoter_one.rs (memory).
//! От чего зависит: check/memory_proof.rs, exec/quoter_refetch_proof.rs (mark), pools/settle_reread.rs.
//! Настройки: нет своих (через вызывающего — dex.quoter_memory_lag_blocks, dex.quoter_check_s).
//! Журнал: dex.quoter_memory — Warn «память неверна на своём блоке … пул на полное перечитывание».

use crate::storage::journal::Level;

use super::super::exec::quoter_refetch_proof::mark;
use super::super::pools::listen::Pools;
use super::super::pools::registry::Entry;
use super::memory_proof::{self, Proof};

/// Вид записи журнала (тот же, что у заметки «память отстаёт от сети»).
const KIND: &str = "dex.quoter_memory";

/// «Память отстаёт» на голове `head`: доказательство на блоке памяти и отметка только при доказанной ошибке (см.
/// шапку). Принимает: пулы, запись из памяти (снимок сверки), голову. Ошибок нет.
pub async fn on_lag(p: &Pools, e: &Entry, head: u64) {
    let Proof::Wrong { note, ticks } = memory_proof::prove(&p.ctx, &p.tokens, &p.reg, e).await
    else {
        return;
    };
    let r = &e.pool.reference;
    let now = p.reg.entry(r).map(|x| (x.synced_block, x.synced_log));
    if mark(ticks, (e.synced_block, e.synced_log), now) && p.holds.reread(&p.reg, r, head) {
        let msg = format!(
            "{} {}: память неверна на своём блоке ({note}) — пул на полное перечитывание сессией пачки, до него не \
             котируется (решение 382)",
            e.family.title, e.ticker
        );
        p.ctx.log(Level::Warn, KIND, msg, None).await;
    }
}

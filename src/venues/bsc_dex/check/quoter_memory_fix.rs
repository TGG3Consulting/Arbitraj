//! Файл: venues/bsc_dex/check/quoter_memory_fix.rs
//! Что делает: решение 382 — «память отстаёт от сети» (решение 261: свежее состояние пула на голове H не равно памяти,
//! последнее применённое событие не позже H − dex.quoter_memory_lag_blocks — check/quoter_block.rs memory_lag). Это ещё
//! не ошибка (событие блока H может быть в пути): доказательство — check/memory_proof.rs (пропущенное событие по логам
//! узла за [S, H] или расхождение с узлом на H при всех применённых логах, ревью 2 Б1); доказана — пул на ПОЛНОЕ чтение
//! своей сессией (pools/settle_reread.rs — до него недостоверен), запись Warn. Отставание / равна / не доказано —
//! только прежняя заметка (dex.quoter_memory, экран «Глубина»). Зацикливания нет: после полного чтения логи учтены и
//! память равна узлу. Одно доказательство на (пул, позиция памяти), доказанная ошибка не повторяется (pools/proof_memo.rs).
//! Частота — не чаще сверки пула (dex.quoter_check_s).
//! Кто вызывает: check/quoter_one.rs (memory).
//! От чего зависит: exec/quoter_refetch_proof.rs (judge), pools/proof_memo.rs.
//! Настройки: нет своих (через вызываемые — dex.quoter_memory_lag_blocks, dex.quoter_check_s).
//! Журнал: dex.quoter_memory — Warn «память неверна … пул на полное перечитывание».

use crate::storage::journal::Level;

use super::super::exec::quoter_refetch_proof::{judge, pos_of};
use super::super::pools::listen::Pools;
use super::super::pools::registry::Entry;
use super::memory_proof::Proof;

/// Вид записи журнала (тот же, что у заметки «память отстаёт от сети»).
const KIND: &str = "dex.quoter_memory";

/// «Память отстаёт» на голове `head` (см. шапку). Принимает: пулы, запись из памяти (снимок сверки), голову, пул,
/// прочитанный на ней. Ошибок нет.
pub async fn on_lag(p: &Pools, e: &Entry, head: u64, at_head: &Entry) {
    if !p.reg.proofs().begin(&e.pool.reference, pos_of(e)) {
        return;
    }
    let (marked, proof) = judge(&p.ctx, (&p.tokens, &p.reg), e, head, Some(at_head)).await;
    if let (true, Proof::Wrong { note, .. }) = (marked, proof) {
        let msg = format!(
            "{} {}: память неверна ({note}) — пул на полное перечитывание сессией пачки, до него не котируется \
             (решение 382)",
            e.family.title, e.ticker
        );
        p.ctx.log(Level::Warn, KIND, msg, None).await;
    }
}

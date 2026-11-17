//! Файл: venues/bsc_dex/check/quoter_memory_fix.rs
//! Что делает: решение 382 — «память отстаёт от сети» (решение 261: свежее состояние пула на голове H не равно памяти,
//! последнее применённое событие не позже H − dex.quoter_memory_lag_blocks — check/quoter_block.rs memory_lag). Это ещё
//! не ошибка (событие блока H может быть в пути): доказательство — check/memory_proof.rs (пропущенное событие по логам
//! узла, сравнение с узлом на H при всех применённых логах; виды «каждым блоком» — только на блоке памяти); доказана —
//! пул на ПОЛНОЕ чтение своей сессией (pools/settle_reread.rs — до него недостоверен), запись Warn. Отставание /
//! равна / не доказано — только прежняя заметка (dex.quoter_memory, экран «Глубина»). Доказательство идёт ФОНОМ
//! (ревью 3 Н4: eth_getLogs на тысячи блоков — до секунды; последовательный проход сверки его не ждёт), одно на
//! (пул, позиция памяти) — pools/proof_memo.rs. Частота — не чаще сверки пула (dex.quoter_check_s).
//! Кто вызывает: check/quoter_one.rs (memory).
//! От чего зависит: exec/quoter_refetch_proof.rs (spawn).
//! Настройки: нет своих (через вызываемые — dex.quoter_memory_lag_blocks, dex.quoter_check_s).
//! Журнал: dex.quoter_memory — Warn «память неверна … пул на полное перечитывание».

use super::super::exec::quoter_refetch_proof;
use super::super::pools::listen::Pools;
use super::super::pools::registry::Entry;

/// Вид записи журнала (тот же, что у заметки «память отстаёт от сети»).
const KIND: &str = "dex.quoter_memory";

/// «Память отстаёт» на голове `head` — доказательство фоном (см. шапку). Принимает: пулы, запись из памяти (снимок
/// сверки), голову, пул, прочитанный на ней. Ошибок нет.
pub fn on_lag(p: &Pools, e: &Entry, head: u64, at_head: &Entry) {
    quoter_refetch_proof::spawn(
        &p.ctx,
        (&p.tokens, &p.reg),
        e,
        (head, at_head),
        (KIND, false),
    );
}

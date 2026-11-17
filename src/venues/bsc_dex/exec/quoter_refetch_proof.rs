//! Файл: venues/bsc_dex/exec/quoter_refetch_proof.rs
//! Что делает: решение 382 (ревью Б1, ревью 2 Б1/Н1/Н2) — доказательство «память пула неверна» (check/memory_proof.rs:
//! пропущенное событие по логам узла за [S, H] или расхождение с узлом на H при всех применённых логах) и отметка «нужно
//! полное чтение» (pools/settle_reread.rs; до него пул недостоверен) — только по доказанной ошибке:
//! - после перечитывания пулов перед сделкой (exec/quoter_refetch.rs pinned) — фоном (spawn: отдельная задача tokio,
//!   сделку не задерживает, класс очереди узла — фон);
//! - по «память отстаёт от сети» сверки (check/quoter_memory_fix.rs) — judge в цикле сверки.
//!
//! Отметка (mark): сильная ошибка (пропуск события, карта тиков) — да; только верх — если память с тех пор не
//! обновлялась; память с момента снимка перечитана целиком (позиция другая, logIndex нет) — нет (Н1: доказательство
//! устарело). Одно доказательство на (пул, позиция памяти) одновременно, уже доказанная ошибка для той же позиции не
//! доказывается снова (pools/proof_memo.rs, Н2).
//! Кто вызывает: exec/quoter_refetch.rs (spawn), check/quoter_memory_fix.rs (judge, mark).
//! От чего зависит: check/memory_proof.rs, pools/settle_reread.rs, pools/proof_memo.rs, pools/registry.rs, tokens.rs.
//! Настройки: через check/memory_proof.rs — dex.quoter_memory_lag_blocks, настройки чтения вида пула.
//! Журнал: dex.memory_refetch — Warn (память неверна, пул на чтение), Info (отставание / верна / не доказано /
//! доказательство устарело).

use crate::storage::journal::Level;

use super::super::check::memory_proof::{self, Proof};
use super::super::ctx::Ctx;
use super::super::pools::proof_memo::Pos;
use super::super::pools::registry::{Entry, Registry};
use super::super::tokens::Tokens;
use super::quoter_refetch::KIND;

/// Позиция памяти записи. Ошибок нет.
pub fn pos_of(e: &Entry) -> Pos {
    (e.synced_block, e.synced_log)
}

/// Запустить доказательство фоном (если по этой позиции памяти оно не идёт и ошибка ещё не доказана). Принимает:
/// окружение, токены, реестр, снимок записи памяти, голову H, пул, прочитанный на H. Ошибок нет.
pub fn spawn(
    ctx: &Ctx,
    (tokens, reg): (&Tokens, &Registry),
    mem: &Entry,
    head: u64,
    at_head: &Entry,
) {
    if !reg.proofs().begin(&mem.pool.reference, pos_of(mem)) {
        return;
    }
    let (ctx, tokens, reg) = (ctx.clone(), tokens.clone(), reg.clone());
    let (mem, at) = (mem.clone(), at_head.clone());
    tokio::spawn(async move {
        let (marked, proof) = judge(&ctx, (&tokens, &reg), &mem, head, Some(&at)).await;
        let title = format!("{} {}", mem.family.title, mem.ticker);
        let (level, text) = match proof {
            Proof::Wrong { note, .. } if marked => (
                Level::Warn,
                format!(
                    "{title}: память неверна ({note}) — пул на полное перечитывание сессией пачки, до него не \
                     котируется (решение 382)"
                ),
            ),
            Proof::Wrong { note, .. } => (
                Level::Info,
                format!(
                    "{title}: память была неверна ({note}), но с тех пор перечитана или обновлена, или пул уже \
                     недостоверен — отметки нет (решение 382)"
                ),
            ),
            Proof::Lag(why) => (
                Level::Info,
                format!("{title}: память отстаёт, не ошибка ({why}) — отметки нет (решение 382)"),
            ),
            Proof::Right => (
                Level::Info,
                format!("{title}: память равна узлу на блоке {head} — отметки нет (решение 382)"),
            ),
            Proof::Unproven(why) => (
                Level::Info,
                format!("{title}: ошибка памяти не доказана ({why}) — отметки нет (решение 382)"),
            ),
        };
        ctx.log(level, KIND, text, None).await;
    });
}

/// Ставить ли отметку по доказанной ошибке (чистая). Принимает: сильная ли ошибка, позицию снимка, позицию сейчас
/// (None — пула нет). Ошибок нет.
pub fn mark(strong: bool, snap: Pos, now: Option<Pos>) -> bool {
    match now {
        None => false,
        // Н1: с момента снимка память перечитана целиком — доказательство устарело.
        Some(n) if n.1.is_none() && n != snap => false,
        Some(n) => strong || n == snap,
    }
}

/// Доказательство и отметка; учёт (пул, позиция) закрывается. Вызывающий открыл его (proof_memo begin). Принимает:
/// окружение, (токены, реестр), снимок памяти, голову, пул на голове (None — прочитать). Возвращает: (отмечен ли,
/// итог). Ошибок нет.
pub async fn judge(
    ctx: &Ctx,
    (tokens, reg): (&Tokens, &Registry),
    mem: &Entry,
    head: u64,
    at_head: Option<&Entry>,
) -> (bool, Proof) {
    let proof = memory_proof::prove(ctx, (tokens, reg), mem, head, at_head).await;
    let r = &mem.pool.reference;
    let marked = match &proof {
        Proof::Wrong { strong, .. } => {
            let now = reg.entry(r).map(|e| pos_of(&e));
            mark(*strong, pos_of(mem), now) && reg.holds().reread(reg, r, head)
        }
        _ => false,
    };
    reg.proofs().finish(r, pos_of(mem), marked);
    (marked, proof)
}

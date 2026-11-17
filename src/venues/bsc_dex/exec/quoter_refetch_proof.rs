//! Файл: venues/bsc_dex/exec/quoter_refetch_proof.rs
//! Что делает: решение 382, ревью Б1 — после перечитывания пулов перед сделкой (exec/quoter_refetch.rs pinned) фоном
//! (отдельная задача tokio — сделку не задерживает, класс очереди узла — фон) доказывает, что память пула неверна, на
//! блоке САМОЙ памяти (check/memory_proof.rs). Только доказанная ошибка ставит пул на полное чтение сессией
//! (pools/settle_reread.rs; до него недостоверен). Ошибка лишь верха состояния (Swap) и с тех пор пул обновлён
//! событием — отметки нет (событие верх уже поправило); ошибка карты тиков (Mint/Burn) — отметка всегда (Swap её не
//! правит). Отставание (память верна на своём блоке) и «не доказано» — строка в журнал, без отметки.
//! Кто вызывает: exec/quoter_refetch.rs.
//! От чего зависит: check/memory_proof.rs, pools/settle_reread.rs, pools/registry.rs, tokens.rs.
//! Настройки: через вызываемые — настройки чтения вида пула.
//! Журнал: dex.memory_refetch — Warn (память неверна, пул на чтение), Info (отставание / не доказано / уже обновлена).

use crate::storage::journal::Level;

use super::super::check::memory_proof::{self, Proof};
use super::super::ctx::Ctx;
use super::super::pools::registry::{Entry, Registry};
use super::super::tokens::Tokens;
use super::quoter_refetch::KIND;

/// Запустить доказательство фоном. Принимает: окружение, токены, реестр, снимок записи памяти, голову H (блок отметки).
/// Ошибок нет.
pub fn spawn(ctx: &Ctx, tokens: &Tokens, reg: &Registry, mem: &Entry, head: u64) {
    let (ctx, tokens, reg, mem) = (ctx.clone(), tokens.clone(), reg.clone(), mem.clone());
    tokio::spawn(async move { run(&ctx, &tokens, &reg, &mem, head).await });
}

/// Ставить ли отметку по доказанной ошибке: карта тиков неверна — да; только верх — если память с тех пор не
/// обновлялась событием (позиция та же). Чистая. Принимает: испорчена ли карта, позиция снимка, позиция сейчас (None —
/// пула нет). Ошибок нет.
pub fn mark(ticks: bool, snap: (u64, Option<u64>), now: Option<(u64, Option<u64>)>) -> bool {
    match now {
        None => false,
        Some(n) => ticks || n == snap,
    }
}

/// Доказательство и отметка (см. шапку). Ошибок нет.
async fn run(ctx: &Ctx, tokens: &Tokens, reg: &Registry, mem: &Entry, head: u64) {
    let r = &mem.pool.reference;
    let title = format!("{} {}", mem.family.title, mem.ticker);
    let (level, text) = match memory_proof::prove(ctx, tokens, reg, mem).await {
        Proof::Wrong { note, ticks } => {
            let now = reg.entry(r).map(|e| (e.synced_block, e.synced_log));
            if mark(ticks, (mem.synced_block, mem.synced_log), now)
                && reg.holds().reread(reg, r, head)
            {
                let t = format!(
                    "{title}: память неверна на своём блоке ({note}) — пул на полное перечитывание сессией пачки, до \
                     него не котируется (решение 382)"
                );
                (Level::Warn, t)
            } else {
                let t = format!(
                    "{title}: память была неверна на своём блоке ({note}), но с тех пор обновлена или пул уже \
                     недостоверен — отметки нет (решение 382)"
                );
                (Level::Info, t)
            }
        }
        Proof::Right => (
            Level::Info,
            format!(
                "{title}: память верна на своём блоке {} — расхождение с головой {head} было отставанием, отметки нет \
                 (решение 382)",
                mem.synced_block
            ),
        ),
        Proof::Unproven(why) => (
            Level::Info,
            format!("{title}: ошибка памяти не доказана ({why}) — отметки нет (решение 382)"),
        ),
    };
    ctx.log(level, KIND, text, None).await;
}

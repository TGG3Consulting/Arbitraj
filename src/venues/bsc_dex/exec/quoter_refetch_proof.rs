//! Файл: venues/bsc_dex/exec/quoter_refetch_proof.rs
//! Что делает: решение 382 (ревью Б1, ревью 2 Б1/Н1/Н2, ревью 3 Н1/Н3/Н4/Н5) — доказательство «память пула неверна»
//! (check/memory_proof.rs) и отметка «нужно полное чтение» (pools/settle_reread.rs; до него пул недостоверен) — только
//! по доказанной ошибке. Всегда фоном (отдельная задача tokio — ни сделку, ни проход сверки не задерживает; класс
//! очереди узла — фон):
//! - после перечитывания пулов перед сделкой (exec/quoter_refetch.rs pinned) — spawn;
//! - по «память отстаёт от сети» сверки (check/quoter_memory_fix.rs) — тот же spawn.
//!
//! Память для доказательства — текущая запись реестра, если её позиция не позже головы (ревью 3 Н5: снимок до чтения
//! на H у оживлённого пула часто ещё без логов блока H), иначе снимок вызывающего. Отметка (mark): сильная ошибка
//! (пропуск события, карта тиков) — да, но пропуск — только если текущая память так и не применила этот лог (ревью 3
//! Н1: лог мог просто опоздать больше допуска); только верх — если память с тех пор не обновлялась; память с момента
//! снимка перечитана целиком — нет (доказательство устарело). Одно доказательство на (пул, позиция памяти), охранник
//! учёта закрывает запись и при панике / отмене (pools/proof_memo.rs).
//! Кто вызывает: exec/quoter_refetch.rs, check/quoter_memory_fix.rs.
//! От чего зависит: check/memory_proof.rs, pools/settle_reread.rs, pools/proof_memo.rs, pools/registry.rs, tokens.rs.
//! Настройки: через check/memory_proof.rs — dex.quoter_memory_lag_blocks, настройки чтения вида пула.
//! Журнал: вид `kind` вызывающего — Warn (память неверна, пул на чтение), Info (отставание / верна / не доказано /
//! доказательство устарело; только у пути сделки — сверка пишет только отметку).

use crate::storage::journal::Level;

use super::super::check::memory_proof::{self, Proof};
use super::super::ctx::Ctx;
use super::super::pools::proof_memo::{Guard, Pos};
use super::super::pools::registry::{Entry, Registry};
use super::super::tokens::Tokens;

/// Позиция памяти записи. Ошибок нет.
pub fn pos_of(e: &Entry) -> Pos {
    (e.synced_block, e.synced_log)
}

/// Запустить доказательство фоном (если по этой позиции памяти оно не идёт и ошибка ещё не доказана). Принимает:
/// окружение, (токены, реестр), снимок записи памяти, голову H, пул, прочитанный на H, (вид записи журнала, писать ли
/// итоги без отметки). Ошибок нет.
pub fn spawn(
    ctx: &Ctx,
    (tokens, reg): (&Tokens, &Registry),
    mem: &Entry,
    (head, at_head): (u64, &Entry),
    (kind, all): (&'static str, bool),
) {
    let r = &mem.pool.reference;
    let mem = reg
        .get(r)
        .filter(|e| e.synced_block <= head)
        .unwrap_or_else(|| mem.clone());
    let Some(guard) = reg.proofs().begin(r, pos_of(&mem)) else {
        return;
    };
    let (ctx, tokens, reg, at) = (ctx.clone(), tokens.clone(), reg.clone(), at_head.clone());
    tokio::spawn(async move {
        let (marked, proof) = judge(&ctx, (&tokens, &reg), &mem, (head, &at), guard).await;
        if let Some((level, text)) = line(&mem, head, marked, proof, all) {
            ctx.log(level, kind, text, None).await;
        }
    });
}

/// Строка журнала по итогу (чистая). Принимает: запись памяти, голову, отмечен ли, итог, писать ли итоги без отметки.
/// Возвращает: уровень и текст; None — писать нечего. Ошибок нет.
pub fn line(
    mem: &Entry,
    head: u64,
    marked: bool,
    proof: Proof,
    all: bool,
) -> Option<(Level, String)> {
    let title = format!("{} {}", mem.family.title, mem.ticker);
    let info = |t: String| all.then_some((Level::Info, t));
    match proof {
        Proof::Wrong { note, .. } if marked => Some((
            Level::Warn,
            format!(
                "{title}: память неверна ({note}) — пул на полное перечитывание сессией пачки, до него не котируется \
                 (решение 382)"
            ),
        )),
        Proof::Wrong { note, .. } => info(format!(
            "{title}: память была неверна ({note}), но с тех пор перечитана или обновлена, или пул уже недостоверен — \
             отметки нет (решение 382)"
        )),
        Proof::Lag(why) => info(format!(
            "{title}: память отстаёт, не ошибка ({why}) — отметки нет (решение 382)"
        )),
        Proof::Right => info(format!(
            "{title}: память равна узлу на блоке {head} — отметки нет (решение 382)"
        )),
        Proof::Unproven(why) => info(format!(
            "{title}: ошибка памяти не доказана ({why}) — отметки нет (решение 382)"
        )),
    }
}

/// Ставить ли отметку по доказанной ошибке (чистая). Принимает: сильная ли ошибка, позицию снимка, позицию сейчас
/// (None — пула нет). Ошибок нет.
pub fn mark(strong: bool, snap: Pos, now: Option<Pos>) -> bool {
    match now {
        None => false,
        // С момента снимка память перечитана целиком — доказательство устарело.
        Some(n) if n.1.is_none() && n != snap => false,
        Some(n) => strong || n == snap,
    }
}

/// Пропуск всё ещё не применён текущей памятью (чистая; ревью 3 Н1). Принимает: текущую запись (None — пула нет),
/// позицию пропуска (None — ошибка доказана не пропуском). Ошибок нет.
pub fn still_missing(now: Option<&Entry>, missed: Option<(u64, u64)>) -> bool {
    match (now, missed) {
        (None, _) => false,
        (Some(_), None) => true,
        (Some(e), Some((b, i))) => !e.has_applied(b, i),
    }
}

/// Доказательство и отметка; учёт (охранник) закрывается при выходе. Принимает: окружение, (токены, реестр), память,
/// (голова, пул на голове), охранника учёта. Возвращает: (отмечен ли, итог). Ошибок нет.
async fn judge(
    ctx: &Ctx,
    (tokens, reg): (&Tokens, &Registry),
    mem: &Entry,
    (head, at_head): (u64, &Entry),
    mut guard: Guard,
) -> (bool, Proof) {
    let proof = memory_proof::prove(ctx, (tokens, reg), mem, head, Some(at_head)).await;
    let r = &mem.pool.reference;
    let marked = match &proof {
        Proof::Wrong { strong, missed, .. } => {
            let now = reg.entry(r);
            still_missing(now.as_deref(), *missed)
                && mark(*strong, pos_of(mem), now.as_deref().map(pos_of))
                && reg.holds().reread(reg, r, head)
        }
        _ => false,
    };
    guard.proven(marked);
    (marked, proof)
}

//! Файл: venues/bsc_dex/check/quoter_memory_fix.rs
//! Что делает: решение 382 — «память отстаёт от сети» (решение 261: свежее состояние пула на голове H не равно памяти,
//! последнее применённое событие не позже H − dex.quoter_memory_lag_blocks — check/quoter_block.rs memory_lag) больше
//! не только запись: пул ставится на ПОЛНОЕ чтение своей сессией пачки (pools/settle_reread.rs — до него недостоверен,
//! не котируется). Частота — не чаще сверки пула (dex.quoter_check_s: этот код зовёт сверка пула, check/quoter_one.rs).
//! Без зацикливания: отметка помнит блок (QuoterSides.memory_reread); если память пула уже прочитана целиком не раньше
//! этого блока, событий после чтения не применено, а на новой голове она снова не равна узлу — события пула между
//! чтением и головой до памяти не дошли: пул недостоверен (вернёт его следующее полное чтение — событие пула при
//! dex.pool_fail_pause_ms, повтор, переподключение), запись Error, повторной отметки нет.
//! Кто вызывает: check/quoter_one.rs (memory).
//! От чего зависит: pools/settle_reread.rs, pools/registry.rs, check/quoter_sides.rs.
//! Настройки: нет своих (через вызывающего — dex.quoter_memory_lag_blocks, dex.quoter_check_s).
//! Журнал: dex.quoter_memory — Warn «пул поставлен на полное перечитывание», Error «после полного перечитывания …».

use crate::storage::journal::Level;

use super::super::pools::listen::Pools;
use super::super::pools::registry::Entry;
use super::quoter_sides::QuoterSides;

/// Вид записи журнала (тот же, что у заметки «память отстаёт от сети»).
const KIND: &str = "dex.quoter_memory";

/// Что делать с «память отстаёт» по отметке прошлого перечитывания.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Act {
    /// Поставить пул на полное чтение.
    Reread,
    /// Отметка стоит, полного чтения после неё ещё не было — ждать.
    Wait,
    /// Прочитан целиком не раньше отметки, событий после не применено, а память снова не равна узлу — недостоверен.
    Stuck,
}

/// Решение по отметке (чистая логика, см. Act). Принимает: блок прошлой отметки (None — не было), последнее
/// применённое событие памяти (блок, logIndex; None — состояние прочитано целиком на блоке). Ошибок нет.
pub fn act(marked: Option<u64>, synced: (u64, Option<u64>)) -> Act {
    match marked {
        None => Act::Reread,
        // Память старше отметки — полного чтения ещё не было (сверка видит пул, только когда он достоверен).
        Some(b) if synced.0 < b => Act::Wait,
        // Прочитан целиком не раньше отметки и событий после чтения нет — а память снова не равна узлу.
        Some(_) if synced.1.is_none() => Act::Stuck,
        // После чтения события применялись, но память снова отстала — новое отставание: снова на чтение (не чаще
        // сверки пула).
        Some(_) => Act::Reread,
    }
}

/// «Память отстаёт» на голове `head`: отметка, ожидание или недостоверность (см. шапку). Принимает: пулы, запись из
/// памяти, голову, сверку сторон (memory_reread меняется). Ошибок нет.
pub async fn on_lag(p: &Pools, e: &Entry, head: u64, now: &mut QuoterSides) {
    let r = &e.pool.reference;
    let title = format!("{} {}", e.family.title, e.ticker);
    match act(now.memory_reread, (e.synced_block, e.synced_log)) {
        Act::Wait => {}
        Act::Reread => {
            if p.holds.reread(&p.reg, r, head) {
                now.memory_reread = Some(head);
                let msg = format!(
                    "{title}: память не равна узлу на блоке {head} — пул поставлен на полное перечитывание сессией \
                     пачки, до него не котируется (решение 382)"
                );
                p.ctx.log(Level::Warn, KIND, msg, None).await;
            }
        }
        Act::Stuck => {
            p.reg.update(r, |x| x.valid = false);
            now.memory_reread = None;
            let msg = format!(
                "{title}: после полного перечитывания (блок {}) память снова не равна узлу на блоке {head} — события \
                 пула между чтением и головой до памяти не дошли (расхождение на одном блоке, не отставание): пул \
                 недостоверен до следующего полного чтения (решение 382)",
                e.synced_block
            );
            p.ctx.log(Level::Error, KIND, msg, None).await;
        }
    }
}

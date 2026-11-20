//! Файл: venues/bsc_dex/pools/fusion_clock.rs
//! Что делает: решение 463 (повторное ревью t236, п. 2) — время головы не получено (node/heads.rs time_since): комиссию
//! THENA Fusion следующего блока на этой голове посчитать нельзя — достоверные пулы пачки недостоверны на этот блок
//! (settle.rs Need::Clock: valid = false, ядру не уходят), но БЕЗ полного чтения (раньше — Need::Fee у всех пулов, по
//! полному чтению на каждый). На следующей голове со временем fusion_head.rs снимает отметку, если комиссия по книге
//! считается (settle_fusion.rs clock_back), иначе — Need::Fee и полное чтение. Спящие пулы (asleep) не трогаются
//! (fusion_head.rs их пропускает). Журнал — одна запись dex.pool на голову.
//! Кто вызывает: pools/fusion_head.rs (on_head).
//! От чего зависит: settle_fusion.rs (clock_hold), listen.rs (Pools), registry.rs, discover.rs (Found), families.rs.
//! Настройки: нет.

use crate::storage::journal::Level;

use super::discover::Found;
use super::families::Proto;
use super::listen::Pools;

/// Время головы `block` не получено (`why`): отметка Need::Clock у достоверных неспящих пулов THENA Fusion пачки без
/// другой отметки и одна запись в журнал. Принимает: окружение, пулы, голову, причину. Ошибок нет.
pub async fn lost(p: &Pools, pools: &[(Found, String)], block: u64, why: &str) {
    let mut n = 0usize;
    for (f, _) in pools {
        let r = &f.reference;
        if f.family.proto != Proto::Algebra || p.holds.pending(r) {
            continue;
        }
        if p.reg.entry(r).is_some_and(|e| e.valid && !e.asleep) {
            p.holds.clock_hold(&p.reg, r, block);
            n += 1;
        }
    }
    if n == 0 {
        return;
    }
    let msg = format!(
        "THENA Fusion: {why} — {n} пулов недостоверны на голове {block} (в котировки и сделки не идут, ядру не уходят); \
         на голове со временем комиссия снова из книги, без полного чтения (решение 463)"
    );
    p.ctx.log(Level::Warn, "dex.pool", msg, None).await;
}

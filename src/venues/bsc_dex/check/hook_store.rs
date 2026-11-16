//! Файл: venues/bsc_dex/check/hook_store.rs
//! Что делает: решение 335 (ревью, Н2/Н3/Н1) — хранение и выбор влияния крючка пула B/C, измеренного НА БЛОКЕ:
//! - put_head: проба на одном блоке (цикл крючков check/hooks_head.rs, проба перед сделкой exec/precheck.rs
//!   hooks_again → check/hook_batch.rs — решения «335 (В1)», «335 (В1, пакет)») и перемер перед паузой
//!   (check/quoter_reprobe.rs, hooks.reprobe_before_pause) пишут итог в PoolFacts.hook (котировки, как раньше) и в
//!   PoolFacts.hook_head (номер H и итог). Итог на блоке СТАРШЕ уже записанного не пишется (задачи пишут без порядка —
//!   более старое измерение не затирает более новое, Н3);
//! - for_verify: влияние для сверки с квотером (check/quoter_verify.rs) — последнее измерение на блоке (hook_head),
//!   если оно есть и итог в PoolFacts.hook «измерено»; иначе PoolFacts.hook (после «335 (В1)» все измерения — на
//!   блоке, настройки hooks.probe_same_block нет);
//! - stale: возраст прежнего влияния больше hooks.keep_impact_max_age_s (Н1, check/hooks_head.rs keep).
//!
//! Кто вызывает: check/hooks_head.rs, check/cycle.rs, check/quoter_reprobe.rs, check/quoter_verify.rs,
//! exec/precheck.rs.
//! От чего зависит: check/state.rs (Checks, PoolFacts), check/hooks_probe.rs (SHORT), core/types.
//! Настройки: hooks.keep_impact_max_age_s [bsc_dex] — необязательная (через check/hooks_head.rs).

use chrono::{DateTime, Utc};

use crate::core::types::{CheckState, HookGroup, HookImpact, PoolCheck, PoolRef};

use super::super::pools::registry::Entry;
use super::state::Checks;

/// Настройка: предел возраста прежнего влияния крючка, когда узел не отдал блок пробы (решение 335, ревью Н1).
pub const KEEP_AGE: &str = "hooks.keep_impact_max_age_s";

/// Записанное измерение на блоке новее этого (чистая). Принимает: блок записанного (None — не было), блок нового.
/// Возвращает: true — новое старше записанного и не пишется. Ошибок нет.
pub fn newer(stored: Option<u64>, block: u64) -> bool {
    stored.is_some_and(|s| s > block)
}

/// Что делает: пишет итог, измеренный на блоке `block`, в PoolFacts.hook (с hook_wait) и hook_head, если записанное
/// на блоке не новее (атомарно под замком хранилища). Принимает: проверки, пул, блок, итог, имя незаданной настройки.
/// Возвращает: записан ли (false — вызывающий не шлёт PoolCheck ядру). Ошибок нет.
pub fn put_head(
    checks: &Checks,
    r: &PoolRef,
    block: u64,
    pc: &PoolCheck,
    wait: Option<String>,
) -> bool {
    let mut done = false;
    checks.update_pool(r, |f| {
        if newer(f.hook_head.as_ref().map(|(b, _)| *b), block) {
            return;
        }
        f.hook = Some(pc.clone());
        f.hook_wait = wait;
        f.hook_head = Some((block, pc.clone()));
        done = true;
    });
    done
}

/// Что делает: влияние крючка для сверки с квотером (см. шапку; было quoter_verify.rs hook_pct). Принимает: проверки,
/// запись. Возвращает: Ok(None) — крючка B/C нет; Ok(Some(влияние)); Err(причина) — влияние
/// ещё не измерено или проверка крючка не прошла. Ошибок сверх этого нет.
pub fn for_verify(checks: &Checks, e: &Entry) -> Result<Option<HookImpact>, String> {
    if e.pool
        .hooks
        .as_ref()
        .is_none_or(|h| h.group == HookGroup::A)
    {
        return Ok(None);
    }
    let f = checks.pool(&e.pool.reference);
    match f.hook.map(|c| c.impact) {
        Some(CheckState::Known(i)) => {
            let head = f.hook_head.and_then(|(_, pc)| match pc.impact {
                CheckState::Known(h) => Some(h),
                _ => None,
            });
            Ok(Some(head.unwrap_or(i)))
        }
        // Не хватает ликвидности на пробу — свойство пула, не провал крючка: причина как есть.
        Some(CheckState::Failed { reason }) if reason.starts_with(super::hooks_probe::SHORT) => {
            Err(reason)
        }
        Some(CheckState::Failed { reason }) => Err(format!("проверка крючка не прошла: {reason}")),
        _ => Err(super::quoter_verify::HOOK_WAIT.into()),
    }
}

/// Возраст измерения больше предела (чистая). Принимает: время измерения (None — неизвестно), «сейчас», предел, с.
/// Возвращает: true — устарело (неизвестное время — устарело). Ошибок нет.
pub fn stale(at: Option<DateTime<Utc>>, now: DateTime<Utc>, max_s: u64) -> bool {
    match at {
        Some(t) => (now - t).num_seconds() > i64::try_from(max_s).unwrap_or(i64::MAX),
        None => true,
    }
}

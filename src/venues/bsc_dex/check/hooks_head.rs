//! Файл: venues/bsc_dex/check/hooks_head.rs
//! Что делает: решение 335 (задача t156) — проба влияния крючка пула V4 группы B/C в цикле крючков (check/cycle.rs).
//! Настройка hooks.probe_same_block = да: наша математика и квотер — на ОДНОМ блоке. Голова H — один eth_blockNumber,
//! пул читается заново на H тем же загрузчиком, что у сверки с квотером (check/quoter_block.rs at_head, в реестр не
//! пишется), математика без крючка — по этому состоянию, квотер — eth_call на тот же H (check/hooks_probe.rs
//! measure_on). Так влияние крючка не содержит разницы «память бота против сети на latest». Узел не отдал голову или
//! состояние H, сбой связи — влияние не перемеряется, действует прежнее (если прежнего нет — «не измерено» с причиной).
//! Не задана или «нет» — проба как до решения 335 (hooks_probe::check_pool_named: память против «latest»).
//! Настройка hooks.requote_after_probe = да (requote): после перемера влияния пул, у которого сторона на паузе сверки
//! с квотером, сверяется сразу (pools/progress.rs requote → check/quoter_cycle.rs), а не через dex.quoter_check_s.
//! Действует на все виды пулов с крючком: крючок есть только у пулов менеджеров V4 — PancakeSwap Infinity CL,
//! Infinity Bin и Uniswap V4 (pools/discover_v4.rs from_key); у остальных видов Pool.hooks = None (разбор —
//! docs/notes/t156-hook-pools-quoter-pause-2026-11-16.md).
//! Кто вызывает: check/cycle.rs (one).
//! От чего зависит: check/hooks_probe.rs, check/quoter_block.rs, check/quoter_compare.rs (node_fault), check/state.rs,
//! pools/listen.rs (Pools), pools/progress.rs (requote), node/call.rs (block_tag).
//! Настройки: hooks.probe_same_block, hooks.requote_after_probe [bsc_dex] — необязательные (docs/SETTINGS.md).

use crate::core::types::{CheckState, HookGroup, PoolCheck, PoolRef, Side};

use super::super::node::call;
use super::super::pools::listen::Pools;
use super::super::pools::registry::Entry;
use super::hooks_probe;
use super::quoter_block::{self, Head};
use super::quoter_compare::node_fault;

/// Настройка: проба влияния крючка на одном блоке H (решение 335, необязательна).
pub const SAME_BLOCK: &str = "hooks.probe_same_block";
/// Настройка: после перемера влияния пул со стороной на паузе сверяется сразу (решение 335, необязательна).
pub const REQUOTE: &str = "hooks.requote_after_probe";

/// Итог пробы для цикла крючков.
#[derive(Debug, Clone)]
pub enum Probe {
    /// У пула нет крючка группы B/C — проверять нечего.
    NoHook,
    /// Влияние не перемерено (узел не отдал блок): прежний итог остаётся, причина — в журнал.
    Keep(String),
    /// Итог пробы и имя незаданной настройки, если проба не прошла из-за неё (решение 75).
    Got(Box<PoolCheck>, Option<String>),
}

/// Что делает: проба влияния крючка пула по настройке hooks.probe_same_block (см. шапку).
/// Принимает: пулы, запись пула из памяти. Возвращает: Probe. Ошибок нет: сбой — причина в итоге.
pub async fn probe(p: &Pools, e: &Entry) -> Probe {
    if !p.ctx.flag(SAME_BLOCK).unwrap_or(false) {
        return match hooks_probe::check_pool_named(&p.ctx, &p.tokens, &p.reg, e).await {
            None => Probe::NoHook,
            Some((pc, wait)) => Probe::Got(Box::new(pc), wait),
        };
    }
    let Some(group) = e.pool.hooks.as_ref().map(|h| h.group) else {
        return Probe::NoHook;
    };
    if group == HookGroup::A {
        return Probe::NoHook;
    }
    let failed = |reason: String, wait: Option<String>| {
        let pc = hooks_probe::pool_check(&p.ctx, e, group, CheckState::Failed { reason });
        Probe::Got(Box::new(pc), wait)
    };
    let (block, fresh) = match quoter_block::at_head(p, e).await {
        Head::Ready { block, entry } => (block, entry),
        Head::NoBlock(why) => return keep(p, e, group, why),
        Head::Fail((why, setting)) => return failed(why, setting),
    };
    let tag = call::block_tag(block);
    match hooks_probe::measure_on(&p.ctx, &p.tokens, &p.reg, &fresh, &tag).await {
        Ok(Ok(i)) => {
            let pc = hooks_probe::pool_check(&p.ctx, e, group, CheckState::Known(i));
            Probe::Got(Box::new(pc), None)
        }
        Ok(Err(reason)) => failed(reason, None),
        Err(err) if node_fault(&err) => keep(p, e, group, format!("блок {block}: {}", err.message)),
        Err(err) => failed(err.message, err.setting),
    }
}

/// Что делает: «не перемерено» — прежний итог остаётся (Keep); прежнего итога нет — «не измерено» с причиной
/// (иначе сверка ждала бы влияния до следующего полного прохода молча). Принимает: пулы, запись, группу, причину.
/// Возвращает: Probe. Ошибок нет.
fn keep(p: &Pools, e: &Entry, group: HookGroup, why: String) -> Probe {
    if p.checks.pool(&e.pool.reference).hook.is_some() {
        return Probe::Keep(why);
    }
    let reason = format!("влияние крючка не измерено на одном блоке: {why}");
    let pc = hooks_probe::pool_check(&p.ctx, e, group, CheckState::Failed { reason });
    Probe::Got(Box::new(pc), None)
}

/// Что делает: решение 335 (в) — при hooks.requote_after_probe = да и стороне пула на паузе сверки с квотером пул
/// ставится на сверку сразу (pools/progress.rs requote). Принимает: пулы, пул. Возвращает: поставлен ли. Ошибок нет.
pub fn requote(p: &Pools, r: &PoolRef) -> bool {
    if !p.ctx.flag(REQUOTE).unwrap_or(false) {
        return false;
    }
    let q = p.checks.pool(r).quoter;
    if !(q.paused(Side::Buy) || q.paused(Side::Sell)) {
        return false;
    }
    p.signals.requote(r.clone());
    true
}

//! Файл: venues/bsc_dex/check/cycle.rs
//! Что делает: цикл проверок крючков (решение 32): влияние крючков пулов V4 группы B/C. Полный проход — раз в
//! hooks.recheck_interval_s; между проходами — «Перепроверить» (только пулы переданных монет) и «новые пулы» (пулы
//! без итога) — check/recheck.rs. Итоги — события PoolCheck ядру. Журнал — только при смене итога пула (прошла ↔ не
//! прошла, сменилась причина без учёта цифр); первый проход — одна сводка, когда ни один пул не читается и все
//! прочитанные проверены. Пул проверяется сразу, как только стал достоверным (check/fresh.rs, «пул готов», решение 65).
//! Налоги — check/tax_cycle.rs, сверка с квотером — check/quoter_cycle.rs.
//! Решение 401 (t186): в том же проходе — проба пула менеджера (V4 / Infinity) через исполнитель v3 каждого кошелька
//! (check/pass_cycle.rs): своего опроса нет, перепроверка — тем же hooks.recheck_interval_s.
//! Решение 335 / «335 (В1)»: проба — check/hooks_head.rs, всегда на одном блоке H (пул, в том числе спящий, читается
//! заново на H); после удачного перемера пул со стороной на паузе сверки сверяется сразу; итог на блоке —
//! check/hook_store.rs (более старый блок не затирает более новый). При запуске (в том числе после обновления) все
//! пулы новые для цикла: каждый пробуется, как только стал достоверным (check/fresh.rs), не дожидаясь
//! hooks.recheck_interval_s; итоги проверок — только в памяти (check/state.rs), прежних процентов после запуска нет.
//! Кто вызывает: pools/market.rs (запуск при подписке).
//! От чего зависит: check/hooks_head.rs, check/hook_store.rs, check/hooks_probe.rs, check/state.rs, check/recheck.rs,
//! check/pass_cycle.rs, check/taxes_util.rs (Sources),
//! pools/listen.rs (Pools), pools/progress.rs.
//! Настройки: hooks.recheck_interval_s (вся система); через hooks_head — hooks.keep_impact_max_age_s [bsc_dex].

use std::collections::HashMap;
use std::time::Duration;

use crate::core::types::{CheckState, PoolRef};
use crate::gateway::events::MarketEvent;
use crate::settings::read::At;
use crate::storage::journal::Level;

use super::super::pools::listen::Pools;
use super::super::pools::progress::reason_key;
use super::super::settings_wait;
use super::fresh::Fresh;
use super::hook_store;
use super::hooks_head::{self, Probe};
use super::quoter_verify::HOOK_WAIT;
use super::recheck::Queue;
use super::taxes_util::Sources;

/// Цикл крючков V4 группы B/C. Работает постоянно; не задан интервал — остановка с записью в журнал.
pub async fn hooks(p: Pools, queue: Queue) {
    let mut fresh = Fresh::default();
    // Решение 401: балансы источников встречного токена для пробы через исполнитель — на проход (taxes_util.rs).
    let mut sources = Sources::default();
    // Итоги первого прохода по пулам (Some(прошла)) — до сводки: ни один пул не читается и все прочитанные проверены.
    let mut first: Option<HashMap<PoolRef, bool>> = Some(HashMap::new());
    let mut full = false;
    let mut changes = settings_wait::watch(&p.ctx);
    loop {
        // Полный проход — раз в hooks.recheck_interval_s; при старте все пулы и так новые для цикла.
        if full {
            sources = Sources::default();
            for r in p.reg.refs() {
                fresh.check(&r, one(&p, &r, &mut first, &sources)).await;
                if p.out.is_closed() {
                    return;
                }
            }
        }
        full = true;
        // Не задана — цикл ждёт настройку и продолжает сам (settings_wait.rs), а не останавливается навсегда.
        let read = || {
            let s = p
                .ctx
                .settings
                .integer("hooks.recheck_interval_s", At::default());
            s.map(Duration::from_secs)
        };
        let Some(d) =
            settings_wait::until_ok(&p.ctx, "Цикл проверок крючков", "dex.check", read).await
        else {
            return;
        };
        let until = tokio::time::Instant::now() + d;
        loop {
            // Подписка на «пул готов» — до прохода по новым пулам: сигнал во время прохода не теряется (решение 65).
            let ready = p.signals.ready.notified();
            tokio::pin!(ready);
            ready.as_mut().enable();
            for r in fresh.due(&p, |_| false) {
                fresh.check(&r, one(&p, &r, &mut first, &sources)).await;
                if p.out.is_closed() {
                    return;
                }
            }
            if first.is_some() && fresh.settled(&p) {
                let all = first.take().unwrap_or_default();
                let ok = all.values().filter(|v| **v).count();
                let msg = format!(
                    "Проверка крючков (первый проход): прошли {ok}, не прошли {}",
                    all.len() - ok
                );
                p.ctx.log(Level::Info, "dex.check", msg, None).await;
            }
            tokio::select! {
                _ = tokio::time::sleep_until(until) => break,
                _ = &mut ready => {}
                // Настройку задали на лету — крючки, не проверенные из-за неё, проверяются сразу.
                Some(c) = changes.next() => {
                    // Пачка изменений — один проход; только крючки, ждущие именно изменённой настройки.
                    let changed = settings_wait::drain(c, &mut changes);
                    for r in p.reg.refs() {
                        let waits = p.checks.pool(&r).hook_wait.is_some_and(|n| changed.contains(&n));
                        if waits {
                            fresh.check(&r, one(&p, &r, &mut first, &sources)).await;
                        }
                    }
                }
                _ = queue.notified() => {
                    let (coins, new_pools) = queue.take();
                    for r in p.reg.refs() {
                        let Some(e) = p.reg.get(&r) else { continue };
                        let fresh_pool = new_pools && p.checks.pool(&r).hook.is_none();
                        if coins.contains(&e.coin) || fresh_pool {
                            fresh.check(&r, one(&p, &r, &mut first, &sources)).await;
                        }
                    }
                }
            }
        }
    }
}

/// Проверка крючка одного пула (нет крючка B/C — проверять нечего). До сводки первого прохода итог идёт в неё
/// (`first`), журнал — при смене итога и только после сводки. Решение 401: до неё — проба пула менеджера через
/// исполнители кошельков с v3 (check/pass_cycle.rs; `sources` — источники встречного токена прохода). Возвращает: была
/// ли проверка (пула нет или он недостоверен — false: пул не отмечается проверенным и проверится, как только станет
/// достоверным). Ошибок нет.
async fn one(
    p: &Pools,
    r: &PoolRef,
    first: &mut Option<HashMap<PoolRef, bool>>,
    sources: &Sources,
) -> bool {
    // Данные пулов отстают (решение 72) — вердикта нет, проверка после снятия отметки.
    if p.lag.busy() {
        return false;
    }
    let Some(e) = p.reg.get(r) else {
        return false;
    };
    if !e.valid {
        return false;
    }
    // Решение 401: шаг пула менеджера через исполнитель кошельков с v3 (пул без крючка B/C — тоже). Future пробы — в
    // куче: две симуляции и поиск источника не раздувают future цикла.
    Box::pin(super::pass_cycle::pool(p, sources, &e)).await;
    // Решение «335 (В1)»: проба на одном блоке H (check/hooks_head.rs).
    let (pc, wait, block) = match hooks_head::probe(p, &e).await {
        Probe::NoHook => return true,
        Probe::Keep(why) => {
            let msg = format!(
                "{} {}: влияние крючка не перемерено ({why}) — действует прежнее",
                e.family.title, e.ticker
            );
            p.signals.once(&p.ctx, Level::Info, "dex.check", msg).await;
            return true;
        }
        Probe::Got(pc, wait, block) => (*pc, wait, block),
    };
    // Решение 335 (ревью Н3): измерено на блоке старше уже записанного — итог не пишется и ядру не уходит.
    if let Some(b) = block
        && hook_store::newer(p.checks.pool(r).hook_head.map(|(s, _)| s), b)
    {
        return true;
    }
    let prev = p.checks.pool(r).hook.map(|c| c.impact);
    let passed = !matches!(pc.impact, CheckState::Failed { .. });
    if let Some(t) = first {
        t.insert(r.clone(), passed);
    } else {
        let msg = match (&prev, &pc.impact) {
            (Some(CheckState::Failed { reason: a }), CheckState::Failed { reason: b })
                if reason_key(a) == reason_key(b) =>
            {
                None
            }
            (_, CheckState::Failed { reason }) => {
                let text = if reason.starts_with(super::hooks_probe::SHORT) {
                    reason.clone()
                } else {
                    format!("крючок не проверен: {reason}")
                };
                Some((Level::Warn, text))
            }
            (Some(CheckState::Failed { .. }), _) => {
                Some((Level::Info, "крючок снова проверен".to_string()))
            }
            _ => None,
        };
        if let Some((level, text)) = msg {
            let msg = format!("{} {}: {text}", e.family.title, e.ticker);
            p.ctx.log(level, "dex.check", msg, None).await;
        }
    }
    // Не прошла из-за незаданной настройки — её имя (из ошибки), чтобы повторить при её задании (решение 75).
    // Измерено на блоке — через hook_store::put_head (не затирает более новый блок).
    match block {
        Some(b) => {
            if !hook_store::put_head(&p.checks, r, b, &pc, wait) {
                return true;
            }
        }
        None => p.checks.update_pool(r, |f| {
            f.hook = Some(pc.clone());
            f.hook_wait = wait;
        }),
    }
    let known = matches!(pc.impact, CheckState::Known(_));
    let _ = p.out.send(MarketEvent::PoolCheck(pc)).await;
    // Сверка с квотером ждала влияния этого крючка — разбудить её (check/quoter_cycle.rs hook_measured).
    if p.checks.pool(r).quoter.any_err(|why| why == HOOK_WAIT) {
        p.signals.pool_ready();
    }
    // Решение 335 (в), с «335 (В1)» всегда: пул со стороной на паузе сверяется сразу — только после удачной пробы
    // (ревью Н5).
    hooks_head::requote(p, r, known);
    true
}

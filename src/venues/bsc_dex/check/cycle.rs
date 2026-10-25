//! Файл: venues/bsc_dex/check/cycle.rs
//! Что делает: цикл проверок крючков (решение 32): влияние крючков пулов V4 группы B/C. Полный проход — раз в
//! hooks.recheck_interval_s; между проходами — «Перепроверить» (только пулы переданных монет) и «новые пулы» (пулы
//! без итога) — check/recheck.rs. Итоги — события PoolCheck ядру. Журнал — только при смене итога пула (прошла ↔ не
//! прошла, сменилась причина без учёта цифр); первый проход — одна сводка, когда ни один пул не читается и все
//! прочитанные проверены. Пул проверяется сразу, как только стал достоверным (check/fresh.rs, «пул готов», решение 65).
//! Налоги — check/tax_cycle.rs, сверка с квотером — check/quoter_cycle.rs.
//! Кто вызывает: pools/market.rs (запуск при подписке).
//! От чего зависит: check/hooks_probe.rs, check/state.rs, check/recheck.rs, pools/listen.rs (Pools), pools/progress.rs.
//! Настройки: hooks.recheck_interval_s (вся система).

use std::collections::HashMap;
use std::time::Duration;

use crate::core::types::{CheckState, PoolRef};
use crate::gateway::events::MarketEvent;
use crate::settings::read::At;
use crate::storage::journal::Level;

use super::super::pools::listen::Pools;
use super::super::pools::progress::reason_key;
use super::fresh::Fresh;
use super::hooks_probe;
use super::quoter_verify::HOOK_WAIT;
use super::recheck::Queue;

/// Цикл крючков V4 группы B/C. Работает постоянно; не задан интервал — остановка с записью в журнал.
pub async fn hooks(p: Pools, queue: Queue) {
    let mut fresh = Fresh::default();
    // Итоги первого прохода по пулам (Some(прошла)) — до сводки: ни один пул не читается и все прочитанные проверены.
    let mut first: Option<HashMap<PoolRef, bool>> = Some(HashMap::new());
    let mut full = false;
    loop {
        // Полный проход — раз в hooks.recheck_interval_s; при старте все пулы и так новые для цикла.
        if full {
            for r in p.reg.refs() {
                fresh.check(&r, one(&p, &r, &mut first)).await;
                if p.out.is_closed() {
                    return;
                }
            }
        }
        full = true;
        let d = match p
            .ctx
            .settings
            .integer("hooks.recheck_interval_s", At::default())
        {
            Ok(s) => Duration::from_secs(s),
            Err(e) => {
                let msg = format!("Цикл проверок крючков остановлен: {}", e.message);
                p.ctx.log(Level::Error, "dex.check", msg, None).await;
                return;
            }
        };
        let until = tokio::time::Instant::now() + d;
        loop {
            // Подписка на «пул готов» — до прохода по новым пулам: сигнал во время прохода не теряется (решение 65).
            let ready = p.signals.ready.notified();
            tokio::pin!(ready);
            ready.as_mut().enable();
            for r in fresh.due(&p, |_| false) {
                fresh.check(&r, one(&p, &r, &mut first)).await;
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
                _ = queue.notified() => {
                    let (tickers, new_pools) = queue.take();
                    for r in p.reg.refs() {
                        let Some(e) = p.reg.get(&r) else { continue };
                        let fresh_pool = new_pools && p.checks.pool(&r).hook.is_none();
                        if tickers.contains(&e.ticker) || fresh_pool {
                            fresh.check(&r, one(&p, &r, &mut first)).await;
                        }
                    }
                }
            }
        }
    }
}

/// Проверка крючка одного пула (нет крючка B/C — проверять нечего). До сводки первого прохода итог идёт в неё
/// (`first`), журнал — при смене итога и только после сводки. Возвращает: была ли проверка (пула нет или он
/// недостоверен — false: пул не отмечается проверенным и проверится, как только станет достоверным). Ошибок нет.
async fn one(p: &Pools, r: &PoolRef, first: &mut Option<HashMap<PoolRef, bool>>) -> bool {
    // Данные пулов отстают (решение 72) — вердикта нет, проверка после снятия отметки.
    if p.lag.get() > 0 {
        return false;
    }
    let Some(e) = p.reg.get(r) else {
        return false;
    };
    if !e.valid {
        return false;
    }
    let Some(pc) = hooks_probe::check_pool(&p.ctx, &p.tokens, &p.reg, &e).await else {
        return true;
    };
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
    p.checks.update_pool(r, |f| f.hook = Some(pc.clone()));
    let _ = p.out.send(MarketEvent::PoolCheck(pc)).await;
    // Сверка с квотером ждала влияния этого крючка — разбудить её (check/quoter_cycle.rs hook_measured).
    if matches!(&p.checks.pool(r).quoter, Some(Err(why)) if why == HOOK_WAIT) {
        p.signals.pool_ready();
    }
    true
}

//! Файл: venues/bsc_dex/check/cycle.rs
//! Что делает: цикл проверок крючков (решение 32): влияние крючков пулов V4 группы B/C. Полный проход — раз в
//! hooks.recheck_interval_s; между проходами — «Перепроверить» (только пулы переданных монет) и «новые пулы» (пулы
//! без итога) — check/recheck.rs. Итоги — события PoolCheck ядру. Журнал — только при смене итога пула (прошла ↔ не
//! прошла, сменилась причина без учёта цифр); первый проход — одна сводка.
//! Налоги — check/tax_cycle.rs, сверка с квотером — check/quoter_cycle.rs.
//! Кто вызывает: pools/market.rs (запуск при подписке).
//! От чего зависит: check/hooks_probe.rs, check/state.rs, check/recheck.rs, pools/listen.rs (Pools), pools/progress.rs.
//! Настройки: hooks.recheck_interval_s (вся система).

use std::time::Duration;

use crate::core::types::{CheckState, PoolRef};
use crate::gateway::events::MarketEvent;
use crate::settings::read::At;
use crate::storage::journal::Level;

use super::super::pools::listen::Pools;
use super::super::pools::progress::reason_key;
use super::hooks_probe;
use super::recheck::Queue;

/// Цикл крючков V4 группы B/C. Работает постоянно; не задан интервал — остановка с записью в журнал.
pub async fn hooks(p: Pools, queue: Queue) {
    let mut first = true;
    loop {
        let (mut ok, mut failed) = (0usize, 0usize);
        for r in p.reg.refs() {
            match one(&p, &r, first).await {
                Some(true) => ok += 1,
                Some(false) => failed += 1,
                None => {}
            }
            if p.out.is_closed() {
                return;
            }
        }
        if first {
            let msg = format!("Проверка крючков (первый проход): прошли {ok}, не прошли {failed}");
            p.ctx.log(Level::Info, "dex.check", msg, None).await;
            first = false;
        }
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
            tokio::select! {
                _ = tokio::time::sleep_until(until) => break,
                _ = queue.notified() => {
                    let (tickers, new_pools) = queue.take();
                    for r in p.reg.refs() {
                        let Some(e) = p.reg.get(&r) else { continue };
                        let fresh = new_pools && p.checks.pool(&r).hook.is_none();
                        if tickers.contains(&e.ticker) || fresh {
                            one(&p, &r, false).await;
                        }
                    }
                }
            }
        }
    }
}

/// Проверка крючка одного пула. Возвращает: Some(прошла ли) или None — проверять нечего (нет крючка B/C,
/// пул недостоверен). Журнал — при смене итога (не в первом проходе). Ошибок нет.
async fn one(p: &Pools, r: &PoolRef, first: bool) -> Option<bool> {
    let e = p.reg.get(r)?;
    if !e.valid {
        return None;
    }
    let pc = hooks_probe::check_pool(&p.ctx, &p.tokens, &p.reg, &e).await?;
    let prev = p.checks.pool(r).hook.map(|c| c.impact);
    let passed = !matches!(pc.impact, CheckState::Failed { .. });
    if !first {
        let msg = match (&prev, &pc.impact) {
            (Some(CheckState::Failed { reason: a }), CheckState::Failed { reason: b })
                if reason_key(a) == reason_key(b) =>
            {
                None
            }
            (_, CheckState::Failed { reason }) => {
                Some((Level::Warn, format!("крючок не проверен: {reason}")))
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
    Some(passed)
}

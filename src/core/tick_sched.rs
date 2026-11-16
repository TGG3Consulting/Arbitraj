//! Файл: core/tick_sched.rs
//! Что делает: ставит пересчёт монеты потока тика и ведёт задачу пересчёта монеты (вынесено из core/tick.rs —
//! ориентир 200 строк). Пересчёт одной монеты идёт отдельной задачей; тики, пришедшие во время пересчёта, сливаются
//! в один повтор (решение 109; логика — чистый план core/tick_plan.rs). После каждого пересчёта — таймер дозревания
//! (решение 318, signal_ripen.rs arm), затем сон до started + signal.eval_min_interval_ms.
//! Решение 322 (signal.pool_event_now = да): событие пула будит этот сон (tokio::select! sleep / Notify); идущий
//! пересчёт не прерывается; направления площадки события пула — первыми; ограничитель —
//! signal.pool_event_min_interval_ms. Не задана — как раньше.
//!
//! Кто вызывает: core/tick.rs (тики стакана и пула), core/feed_rest_book.rs (через tick::schedule — t79),
//! core/signal_ripen.rs (schedule_set — решение 318).
//! От чего зависит: engine.rs, tick_plan.rs, evaluate.rs, signal_ripen.rs, settings.
//! Настройки: signal.eval_min_interval_ms (решение 109), signal.pool_event_now, signal.pool_event_min_interval_ms
//! (решение 322).

use std::collections::HashSet;
use std::time::{Duration, Instant};

use chrono::Utc;

use crate::core::types::{Asset, VenueId};
use crate::settings::read::At;

use super::engine::{Core, lk};
use super::tick_plan::{Cfg, Next, PoolMark, Put, Run};

/// Ставит пересчёт монеты по тику площадки (стакан биржи, REST-стакан). Ошибок нет.
/// Решение 109: пересчёт одной монеты — не чаще signal.eval_min_interval_ms (не задано — без предела): тики, пришедшие
/// между пересчётами, сливаются в один повтор по всем приславшим площадкам. Так нагрузка зависит от числа монет и
/// этого интервала, а не от того, как часто биржи шлют стаканы (MEXC — снимками потоком).
pub(super) fn schedule(core: &Core, asset: Asset, venue: VenueId) {
    // Тик монеты — тем, кто ждёт её цены (режим «Ожидание», stuck.rs).
    let _ = core.inner.coin_ticks.send(asset.clone());
    put(core, asset, [venue].into(), None);
}

/// Ставит пересчёт монеты по событию пула (решение 322: отметка — время прихода в ядро и блок). Ошибок нет.
pub(super) fn schedule_pool(core: &Core, asset: Asset, venue: VenueId, mark: PoolMark) {
    let _ = core.inner.coin_ticks.send(asset.clone());
    put(core, asset, [venue].into(), Some(mark));
}

/// Ставит пересчёт монеты по площадкам (без отметки тика монеты — так ставит и таймер дозревания, решение 318: нового
/// тика не было). Идёт пересчёт — площадки в повтор; нет — задача. Ошибок нет.
pub(super) fn schedule_set(core: &Core, asset: Asset, venues: HashSet<VenueId>) {
    put(core, asset, venues, None);
}

/// Тик в план: новая задача, повтор или пробуждение спящей задачи (решение 322). Ошибок нет.
fn put(core: &Core, asset: Asset, venues: HashSet<VenueId>, pool: Option<PoolMark>) {
    let pool_now = pool.is_some() && cfg(core).pool_now;
    let put = lk(&core.inner.ticking).put(&asset, venues, pool, pool_now, Instant::now());
    match put {
        Put::Queued => {}
        Put::Wake(w) => w.notify_one(),
        Put::Spawn(run) => {
            tokio::spawn(task(core.clone(), asset, run));
        }
    }
}

/// Задача пересчёта монеты: пересчёт → таймер дозревания → сон или повтор, пока есть тики. Ошибок нет.
async fn task(core: Core, asset: Asset, mut run: Run) {
    loop {
        let started = Instant::now();
        super::evaluate::evaluate(&core, &asset, &run, Utc::now()).await;
        // Решение 318: «держится» без новых тиков — таймер на срок дозревания (не задан signal.hold_timer — нет).
        super::signal_ripen::arm(&core, &asset);
        loop {
            let c = cfg(&core);
            let deadline = c.min.map(|m| started + m);
            let (next, wake) = {
                let mut t = lk(&core.inner.ticking);
                let next = t.next(&asset, deadline, c, Instant::now());
                let wake = t.wake(&asset);
                (next, wake)
            };
            match (next, wake) {
                (Next::Go(r), _) => {
                    run = r;
                    break;
                }
                (Next::Done, _) => return,
                (Next::Sleep(until), Some(w)) => {
                    // Решение 322: событие пула будит сон раньше срока (без signal.pool_event_now не будит никто).
                    tokio::select! {
                        _ = tokio::time::sleep_until(until.into()) => {}
                        _ = w.notified() => {}
                    }
                }
                (Next::Sleep(until), None) => tokio::time::sleep_until(until.into()).await,
            }
        }
    }
}

/// Настройки плана: signal.eval_min_interval_ms (решение 109; не задан или 0 — None), signal.pool_event_now (не задана —
/// нет), signal.pool_event_min_interval_ms (решение 322; не задан или 0 — None). Ошибок нет.
fn cfg(core: &Core) -> Cfg {
    let s = &core.inner.deps.settings;
    let ms = |name: &str| {
        s.integer(name, At::default())
            .ok()
            .filter(|ms| *ms > 0)
            .map(Duration::from_millis)
    };
    Cfg {
        min: ms("signal.eval_min_interval_ms"),
        pool_now: s
            .flag("signal.pool_event_now", At::default())
            .unwrap_or(false),
        pool_min: ms("signal.pool_event_min_interval_ms"),
    }
}

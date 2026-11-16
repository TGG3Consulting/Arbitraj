//! Файл: core/tick_plan.rs
//! Что делает: план пересчётов монет потока тика (чистая логика, время — параметром). По монете не больше одного
//! пересчёта за раз; тики, пришедшие во время пересчёта или сна до started + signal.eval_min_interval_ms, копятся
//! в повтор и сливаются в один пересчёт по всем приславшим площадкам (решение 109).
//! Решение 322 (signal.pool_event_now = да): событие пула будит сон задачи монеты — пересчёт начинается сразу, а не
//! после сна; идущий пересчёт не прерывается (событие ждёт его конца в повторе). Направления с площадками, приславшими
//! событие пула, считаются первыми (Run.first). Ограничитель — signal.pool_event_min_interval_ms: пересчёты, запущенные
//! событием пула раньше срока сна, не чаще этого (от начала прошлого пересчёта с событием пула); не задана — без
//! ограничения сверх одного пересчёта за раз. signal.pool_event_now не задана — побайтно прежний порядок (решение 109).
//! Отметка события пула (время прихода в ядро, номер блока) едет с пересчётом в запись core.signal_hold.
//!
//! Кто вызывает: core/tick_sched.rs (задача пересчёта монеты), tests/core_tick_plan_322.rs.
//! От чего зависит: core/types, tokio (Notify), chrono.
//! Настройки: читает core/tick_sched.rs — signal.eval_min_interval_ms, signal.pool_event_now,
//! signal.pool_event_min_interval_ms (сюда — Cfg).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use tokio::sync::Notify;

use crate::core::types::{Asset, VenueId};

/// Событие пула, пришедшее в ядро: когда его получил поток тика и номер блока состояния пула.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PoolMark {
    pub at: DateTime<Utc>,
    pub block: u64,
}

/// Настройки плана на момент решения (tick_sched.rs читает их на каждый шаг — меняются на лету).
#[derive(Debug, Clone, Copy, Default)]
pub struct Cfg {
    /// signal.eval_min_interval_ms (None — не задан или 0).
    pub min: Option<Duration>,
    /// signal.pool_event_now (не задана — нет).
    pub pool_now: bool,
    /// signal.pool_event_min_interval_ms (None — не задан или 0).
    pub pool_min: Option<Duration>,
}

/// Пересчёт к запуску.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Run {
    /// Площадки, приславшие тики (пересчитываются направления, где они участвуют).
    pub venues: HashSet<VenueId>,
    /// Решение 322: площадки, приславшие событие пула, — их направления первыми (пусто — порядок прежний).
    pub first: HashSet<VenueId>,
    /// Первое событие пула этого пересчёта (None — пересчёт без события пула).
    pub pool: Option<PoolMark>,
}

/// Что сделать с новым тиком.
#[derive(Debug)]
pub enum Put {
    /// Пересчёта нет — запустить задачу с этим пересчётом.
    Spawn(Run),
    /// Пересчёт идёт или задача спит — тик в повторе.
    Queued,
    /// Решение 322: задача спит, пришло событие пула — разбудить её.
    Wake(Arc<Notify>),
}

/// Что делать задаче после пересчёта или сна.
#[derive(Debug, PartialEq)]
pub enum Next {
    Go(Run),
    Sleep(Instant),
    Done,
}

/// Монета, по которой идёт задача пересчёта.
#[derive(Default)]
struct Slot {
    pending: HashSet<VenueId>,
    pool_venues: HashSet<VenueId>,
    pool: Option<PoolMark>,
    /// Начало последнего пересчёта с событием пула (ограничитель решения 322).
    last_pool_run: Option<Instant>,
    sleeping: bool,
    wake: Arc<Notify>,
}

/// План по всем монетам.
#[derive(Default)]
pub struct Plan {
    slots: HashMap<Asset, Slot>,
}

impl Plan {
    /// Что делает: принимает тик монеты. Принимает: монету, площадки тика, отметку события пула (Some — все площадки
    /// тика прислали событие пула), включено ли решение 322, время. Возвращает: Put. Ошибок нет.
    pub fn put(
        &mut self,
        asset: &Asset,
        venues: HashSet<VenueId>,
        pool: Option<PoolMark>,
        pool_now: bool,
        now: Instant,
    ) -> Put {
        if let Some(s) = self.slots.get_mut(asset) {
            if pool.is_some() {
                s.pool_venues.extend(venues.iter().cloned());
                s.pool = s.pool.or(pool);
            }
            s.pending.extend(venues);
            return if pool.is_some() && pool_now && s.sleeping {
                Put::Wake(s.wake.clone())
            } else {
                Put::Queued
            };
        }
        let mut slot = Slot::default();
        if pool.is_some() {
            slot.last_pool_run = Some(now);
        }
        self.slots.insert(asset.clone(), slot);
        let first = if pool.is_some() && pool_now {
            venues.clone()
        } else {
            HashSet::new()
        };
        Put::Spawn(Run {
            venues,
            first,
            pool,
        })
    }

    /// Что делает: решает, что делать задаче монеты дальше. Принимает: монету, срок сна (started +
    /// signal.eval_min_interval_ms; None — без предела), настройки, время. Возвращает: Go — следующий пересчёт
    /// (повтор забран); Sleep — спать до срока (событие пула при решении 322 будит раньше); Done — повтора нет, срок
    /// прошёл: задача кончается, монета снята. Ошибок нет.
    pub fn next(
        &mut self,
        asset: &Asset,
        deadline: Option<Instant>,
        cfg: Cfg,
        now: Instant,
    ) -> Next {
        let Some(s) = self.slots.get_mut(asset) else {
            return Next::Done;
        };
        let due = deadline.is_none_or(|d| now >= d);
        if s.pending.is_empty() {
            return match deadline {
                Some(d) if !due => {
                    s.sleeping = true;
                    Next::Sleep(d)
                }
                _ => {
                    self.slots.remove(asset);
                    Next::Done
                }
            };
        }
        let early = cfg.pool_now && !s.pool_venues.is_empty();
        let ok_at = match (cfg.pool_min, s.last_pool_run) {
            (Some(m), Some(l)) => l + m,
            _ => now,
        };
        // Срок сна прошёл — повтор; решение 322 — событие пула в повторе и ограничитель пройден — повтор раньше срока.
        let go = due || (early && now >= ok_at);
        if !go {
            let until = match deadline {
                Some(d) if early => d.min(ok_at),
                Some(d) => d,
                None => now,
            };
            s.sleeping = true;
            return Next::Sleep(until);
        }
        s.sleeping = false;
        let pool = s.pool.take();
        let pool_venues = std::mem::take(&mut s.pool_venues);
        if pool.is_some() {
            s.last_pool_run = Some(now);
        }
        Next::Go(Run {
            venues: std::mem::take(&mut s.pending),
            first: if cfg.pool_now {
                pool_venues
            } else {
                HashSet::new()
            },
            pool,
        })
    }

    /// Что делает: сигнал пробуждения задачи монеты (решение 322: его ждёт сон задачи). Возвращает: None — задачи
    /// нет. Ошибок нет.
    pub fn wake(&self, asset: &Asset) -> Option<Arc<Notify>> {
        self.slots.get(asset).map(|s| s.wake.clone())
    }
}

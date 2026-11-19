//! Файл: venues/kraken/standby_log.rs
//! Что делает: решение 426 — счётчики страховки стакана Kraken (на адаптер, с запуска) и запись журнала
//! `venue.book_standby` (Warn): сколько раз стакан пары переключался на страховку и обратно на основной, сколько раз
//! основной и страховка теряли стакан пары (пересинхронизация: переподписка или обрыв соединения), сколько раз обе
//! стороны были недостоверны («недостоверен» ядру). Запись — не чаще раза в log.pool_events_lost_interval_s секунд
//! (общие часы журнала gateway/log_clock.rs, та же настройка, что у venue.book_events_lost решения 324; первая —
//! сразу); не задана — записей нет, счёт копится и уйдёт первой записью.
//! Кто вызывает: ws_conn.rs, ws.rs (через standby_emit.rs).
//! От чего зависит: gateway/log_clock.rs, standby.rs (Role, Switch), ctx.rs (настройка), storage/journal.
//! Настройки: log.pool_events_lost_interval_s (необязательная, вся система / площадка).

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use crate::gateway::log_clock::Clock;

use super::standby::{Role, Switch};

/// Что случилось с парой.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    Switched(Switch),
    /// Соединение этой роли потеряло стакан пары.
    Lost(Role),
    /// Обе стороны недостоверны — ядру «недостоверен».
    Blind,
}

/// Счётчик: с прошлой записи и с запуска.
#[derive(Default)]
struct Count {
    since: AtomicU64,
    total: AtomicU64,
}

impl Count {
    /// +1. Ошибок нет.
    fn bump(&self) {
        self.since.fetch_add(1, Ordering::Relaxed);
        self.total.fetch_add(1, Ordering::Relaxed);
    }

    /// (с прошлой записи — обнуляется, с запуска). Ошибок нет.
    fn take(&self) -> (u64, u64) {
        (
            self.since.swap(0, Ordering::Relaxed),
            self.total.load(Ordering::Relaxed),
        )
    }
}

/// Счётчики адаптера. Дешёво разделяется (Arc в book.rs Books).
#[derive(Default)]
pub struct Tally {
    to_standby: Count,
    to_main: Count,
    main_lost: Count,
    standby_lost: Count,
    blind: Count,
    clock: Mutex<Clock>,
}

impl Tally {
    /// Что делает: учитывает событие и, если пора, возвращает текст записи журнала (счётчики с прошлой записи
    /// обнуляются). Принимает: событие, чтение интервала (секунды; None — не задан). Возвращает: текст или None.
    /// Ошибок нет.
    pub fn note(&self, e: Event, every_s: impl FnOnce() -> Option<u64>) -> Option<String> {
        match e {
            Event::Switched(Switch::ToStandby) => self.to_standby.bump(),
            Event::Switched(Switch::ToMain) => self.to_main.bump(),
            Event::Lost(Role::Main) => self.main_lost.bump(),
            Event::Lost(Role::Standby) => self.standby_lost.bump(),
            Event::Blind => self.blind.bump(),
        }
        let since = self
            .clock
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .due(Instant::now(), || {
                every_s().filter(|s| *s > 0).map(Duration::from_secs)
            })?;
        let window = since.map_or("с запуска".to_string(), |d| {
            format!("за {} с", d.as_secs())
        });
        let f = |c: &Count| {
            let (s, t) = c.take();
            format!("{s} (с запуска {t})")
        };
        Some(format!(
            "Kraken, страховка стакана (решение 426) {window}: на страховку {}, обратно на основной {}, \
             основной терял стакан пары {}, страховка теряла {}, обе недостоверны {}",
            f(&self.to_standby),
            f(&self.to_main),
            f(&self.main_lost),
            f(&self.standby_lost),
            f(&self.blind)
        ))
    }
}

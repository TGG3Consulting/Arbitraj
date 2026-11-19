//! Файл: venues/htx/fast_log.rs
//! Что делает: решение 422 — счёт переключений стакана HTX «быстрый поток ↔ снимки», пересинхронизаций быстрого
//! стакана и непринятых подписок быстрого канала; одна сводная запись журнала venue.book_fast (уровень Info) не чаще
//! раза в log.pool_events_lost_interval_s секунд (часы gateway/log_clock.rs — тот же механизм и та же настройка, что у
//! записи стаканов venue.book_events_lost, решение 324; новых настроек журнала нет). Не задана — записей нет, счёт
//! копится и уйдёт первой записью. Запись — только по событию (переключение, разрыв), не по кадру.
//! Кто вызывает: book_mode.rs (счёт), fast_conn.rs, fast_pair.rs и ws_conn.rs (flush после события).
//! От чего зависит: gateway/log_clock.rs, ctx.rs, storage/journal.rs.
//! Настройки: log.pool_events_lost_interval_s [вся система / площадка].

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use crate::gateway::log_clock::Clock;
use crate::storage::journal::Level;

use super::ctx::Ctx;

/// Счётчики с прошлой записи и часы записи.
#[derive(Default)]
pub struct FastLog {
    to_fast: AtomicU64,
    to_snap: AtomicU64,
    resync: AtomicU64,
    rejected: AtomicU64,
    /// Последняя причина (переключения на снимки, разрыва или отказа подписки).
    last: Mutex<String>,
    clock: Mutex<Clock>,
}

impl FastLog {
    /// Пара вернулась на быстрый стакан. Ошибок нет.
    pub fn to_fast(&self) {
        self.to_fast.fetch_add(1, Ordering::Relaxed);
    }

    /// Пара переключена на снимки mbp.refresh. Принимает: причину. Ошибок нет.
    pub fn to_snap(&self, why: &str) {
        self.to_snap.fetch_add(1, Ordering::Relaxed);
        self.reason(why);
    }

    /// Пересинхронизация быстрого стакана пары (новый запрос снимка). Принимает: причину. Ошибок нет.
    pub fn resync(&self, why: &str) {
        self.resync.fetch_add(1, Ordering::Relaxed);
        self.reason(why);
    }

    /// Подписка быстрого канала пары не принята (у 5/20 уровней — список пар в документации). Принимает: причину.
    /// Ошибок нет.
    pub fn rejected(&self, why: &str) {
        self.rejected.fetch_add(1, Ordering::Relaxed);
        self.reason(why);
    }

    /// Запоминает последнюю причину. Ошибок нет.
    fn reason(&self, why: &str) {
        *self.last.lock().unwrap_or_else(|p| p.into_inner()) = why.to_string();
    }

    /// Если пора и есть что сказать — сводная запись журнала (счёт обнуляется). Принимает: окружение HTX. Ошибок нет.
    pub async fn flush(&self, ctx: &Ctx) {
        let load = |c: &AtomicU64| c.load(Ordering::Relaxed);
        if load(&self.to_fast) + load(&self.to_snap) + load(&self.resync) + load(&self.rejected)
            == 0
        {
            return;
        }
        let due = self
            .clock
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .due(Instant::now(), || {
                ctx.int("log.pool_events_lost_interval_s")
                    .ok()
                    .map(Duration::from_secs)
            });
        let Some(since) = due else {
            return;
        };
        let take = |c: &AtomicU64| c.swap(0, Ordering::Relaxed);
        let (f, s, r, x) = (
            take(&self.to_fast),
            take(&self.to_snap),
            take(&self.resync),
            take(&self.rejected),
        );
        let last = self.last.lock().unwrap_or_else(|p| p.into_inner()).clone();
        let window = since.map_or("с запуска".to_string(), |d| {
            format!("за {} с", d.as_secs())
        });
        let msg = format!(
            "быстрый стакан (решение 422) {window}: на снимки {s}, обратно на быстрый {f}, пересинхронизаций {r}, \
             подписок не принято {x}; последняя причина: {last}"
        );
        ctx.log(Level::Info, "venue.book_fast", msg).await;
    }
}

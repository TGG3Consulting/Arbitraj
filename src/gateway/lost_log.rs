//! Файл: gateway/lost_log.rs
//! Что делает: решение 334 (задача t155) — ни одна потеря сообщения или кандидата не молчит: общий счёт потерь одного
//! места (атомарный) и запись журнала уровня Warn о нём. Без настройки log.events_lost_interval_s — запись на 1-ю, 2-ю,
//! 4-ю, 8-ю… потерю (как у очередей окон и эпизодов, core/window_run.rs, core/episode_run.rs: шквал потерь даёт
//! несколько строк, а не тысячи); задана — не чаще раза в столько секунд (первая — сразу) с числом потерь с прошлой
//! записи. Счёт и настройка трогаются только при потере (обычный путь отправки — без изменений).
//! Кто вызывает: gateway/core_out_must_log.rs (сообщения адаптеров ядру), core/cand_wait.rs и core/candidates.rs
//! (кандидаты), core/busy_calc.rs (теневые расчёты), core/stuck_lag.rs (тики монет), notify/mirror_lag.rs (события
//! ядра).
//! От чего зависит: gateway/log_clock.rs (часы «не чаще интервала»), settings (интервал), storage/journal.rs, tokio
//! (запись отдельной задачей — вызывающий не ждёт).
//! Настройки: log.events_lost_interval_s (вся система; необязательная).

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde_json::json;

use crate::settings::read::At;
use crate::settings::store::Store;
use crate::storage::journal::{Journal, Level};

use super::log_clock::Clock;

/// Интервал записи журнала о потерях, секунды (решение 334).
pub const INTERVAL: &str = "log.events_lost_interval_s";

/// Счёт потерь одного места. Default — ноль потерь.
#[derive(Debug, Default)]
pub struct Lost {
    /// Всего потерь с запуска.
    total: AtomicU64,
    /// Потерь с прошлой записи журнала.
    since: AtomicU64,
    /// Часы записи при заданном интервале.
    clock: Mutex<Clock>,
}

/// Что писать: вид записи журнала, площадка (None — без площадки) и что потеряно (для текста).
pub struct Note<'a> {
    pub kind: &'static str,
    pub venue: Option<&'a str>,
    /// Что теряется и чем грозит, например «кандидат не посчитан (очередь расчётов полна)».
    pub what: &'a str,
    /// Последнее потерянное (тикер, код события) — для разбора; пусто — не пишется.
    pub last: String,
}

impl Lost {
    /// Всего потерь с запуска. Ошибок нет.
    pub fn total(&self) -> u64 {
        self.total.load(Ordering::Relaxed)
    }

    /// Одна потеря: счёт и, если пора, запись журнала (отдельной задачей tokio). Принимает: настройки, журнал, что
    /// писать. Возвращает: всего потерь с запуска. Ошибок нет; нет рантайма tokio — только счёт.
    pub fn hit(&self, settings: &Store, journal: &Journal, note: Note<'_>) -> u64 {
        let total = self.total.fetch_add(1, Ordering::Relaxed) + 1;
        self.since.fetch_add(1, Ordering::Relaxed);
        let every = settings
            .integer(INTERVAL, At::default())
            .ok()
            .filter(|s| *s > 0)
            .map(Duration::from_secs);
        // Some(с прошлой записи) — пора писать.
        let due: Option<Option<Duration>> = match every {
            Some(e) => self
                .clock
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .due(Instant::now(), || Some(e)),
            None => total.is_power_of_two().then_some(None),
        };
        let Some(window) = due else {
            return total;
        };
        let since = self.since.swap(0, Ordering::Relaxed);
        let when = match (every, window) {
            (Some(_), Some(d)) => format!("за {} с", d.as_secs()),
            (Some(_), None) => "с запуска".to_string(),
            (None, _) => "с прошлой записи (запись на 1-й, 2-й, 4-й… потере)".to_string(),
        };
        let last = if note.last.is_empty() {
            String::new()
        } else {
            format!("; последнее: {}", note.last)
        };
        let msg = format!(
            "{}: потеряно {since} {when}, всего с запуска {total}{last}",
            note.what
        );
        let data = json!({
            "lost": since, "total": total, "since_s": window.map(|d| d.as_secs()), "last": note.last,
        });
        let Ok(rt) = tokio::runtime::Handle::try_current() else {
            return total;
        };
        let (journal, kind, venue) = (journal.clone(), note.kind, note.venue.map(str::to_string));
        rt.spawn(async move {
            let _ = journal
                .write(Level::Warn, kind, venue.as_deref(), msg, Some(data))
                .await;
        });
        total
    }
}

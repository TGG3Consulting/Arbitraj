//! Счётчики быстрого стакана MEXC. На активную UTC-минуту процесса создаётся одна
//! повторяющаяся строка. Верхние поля JSON остаются накопительными (решение 420);
//! minute_counts суммируется по minute_start_utc_s и разным ID строк, в том числе после рестарта.
//! Запись выполняется через очередь журнала без ожидания SQLite (решение 354).

use std::collections::VecDeque;
use std::sync::{LazyLock, Mutex};

use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::json;

use crate::storage::journal::{Journal, Level};

use super::ctx::ID;

/// События состояния быстрых книг (все пары процесса).
#[derive(Clone, Copy)]
pub enum Ev {
    ToSnap,
    ToFast,
    Resync,
    SnapFail,
}

#[derive(Clone, Copy, Default, Serialize)]
struct Counts {
    to_snap: u64,
    to_fast: u64,
    resyncs: u64,
    snapshot_failed: u64,
}

impl Counts {
    fn add(&mut self, ev: Ev) -> &'static str {
        let (count, label) = match ev {
            Ev::ToSnap => (&mut self.to_snap, "на снимки"),
            Ev::ToFast => (&mut self.to_fast, "на быстрый"),
            Ev::Resync => (&mut self.resyncs, "пересинхронизация"),
            Ev::SnapFail => (&mut self.snapshot_failed, "снимок REST не получен"),
        };
        *count += 1;
        label
    }

    fn repeats(&self) -> i64 {
        (self.to_snap + self.to_fast + self.resyncs + self.snapshot_failed) as i64
    }
}

#[derive(Clone, Copy, Default)]
struct Minute {
    utc_s: i64,
    counts: Counts,
    process: Counts,
}

impl Minute {
    fn fields(
        &self,
        recent: Option<(&str, &str, &str)>,
        coverage: Coverage,
    ) -> (String, serde_json::Value) {
        let p = self.process;
        let m = self.counts;
        let minute_of_day = self.utc_s.rem_euclid(86_400) / 60;
        let mut msg = format!(
            "MEXC быстрый стакан за {:02}:{:02} UTC: повторно поставлено снимков REST {}, сбоев снимка {}; с запуска процесса (все пары): на снимки {}, обратно на быстрый {}, пересинхронизаций {}, сбоев снимка {}",
            minute_of_day / 60,
            minute_of_day % 60,
            m.resyncs,
            m.snapshot_failed,
            p.to_snap,
            p.to_fast,
            p.resyncs,
            p.snapshot_failed
        );
        if let Some((ticker, what, why)) = recent {
            use std::fmt::Write;
            let _ = write!(msg, "; последнее — {ticker}: {what}: {why}");
        }
        (
            msg,
            json!({
                "to_snap":p.to_snap, "to_fast":p.to_fast, "resyncs":p.resyncs, "snapshot_failed":p.snapshot_failed,
                "counter_scope":"process", "ticker":recent.map(|v|v.0),
                "minute_start_utc_s":self.utc_s, "minute_counts":m,
                "unwritten_closed_minutes":coverage.pending, "evicted_unwritten_minutes":coverage.evicted,
            }),
        )
    }

    fn insert(
        &self,
        j: &Journal,
        recent: Option<(&str, &str, &str)>,
        coverage: Coverage,
    ) -> Option<u64> {
        let (msg, data) = self.fields(recent, coverage);
        let n = self.counts.repeats();
        // В обычном случае это единственная операция, n=1. После отказа первоначальной
        // вставки repeats должен включить и события, уже накопленные именно в этой минуте.
        j.counted_queued_with_repeats(Level::Warn, "venue.fast_book", Some(ID), msg, Some(data), n)
    }
}

// Память на случай отказа регистрации строки ограничена одними сутками активных минут.
const RETAIN_MINUTES: usize = 24 * 60;

#[derive(Clone, Copy)]
struct Coverage {
    pending: usize,
    evicted: u64,
}

#[derive(Default)]
struct St {
    process: Counts,
    active: Option<Minute>,
    token: Option<u64>,
    unsaved: VecDeque<Minute>,
    evicted: u64,
}

impl St {
    fn note(&mut self, j: &Journal, at: DateTime<Utc>, ev: Ev, ticker: &str, why: &str) {
        let minute = at.timestamp().div_euclid(60) * 60;
        if self.active.is_some_and(|m| m.utc_s != minute) {
            if let Some(token) = self.token.take() {
                j.forget_queued(token);
            } else if let Some(previous) = self.active {
                if self.unsaved.len() == RETAIN_MINUTES {
                    self.unsaved.pop_front();
                    self.evicted += 1;
                }
                self.unsaved.push_back(previous);
            }
            self.active = None;
        }
        self.process.add(ev);
        let current = self.active.get_or_insert_with(|| Minute {
            utc_s: minute,
            ..Default::default()
        });
        current.process = self.process;
        let what = current.counts.add(ev);
        // Максимум одна старая минута на событие. При заполненной очереди не делаем
        // вторую заведомо неудачную вставку: новое событие уже учтено в своей минуте.
        let mut coverage = Coverage {
            pending: self.unsaved.len(),
            evicted: self.evicted,
        };
        if let Some(previous) = self.unsaved.front() {
            coverage.pending -= 1;
            let Some(t) = previous.insert(j, None, coverage) else {
                return;
            };
            j.forget_queued(t);
            self.unsaved.pop_front();
        }
        let recent = Some((ticker, what, why));
        match self.token {
            None => self.token = current.insert(j, recent, coverage),
            Some(t) => {
                let (msg, data) = current.fields(recent, coverage);
                j.update_queued(t, current.counts.repeats(), at, msg, Some(data));
            }
        }
    }
}

static STATS: LazyLock<Mutex<St>> = LazyLock::new(|| Mutex::new(St::default()));

pub fn note(journal: &Journal, ev: Ev, ticker: &str, why: &str) {
    let mut state = STATS.lock().unwrap_or_else(|p| p.into_inner());
    state.note(journal, Utc::now(), ev, ticker, why);
}

#[cfg(test)]
#[path = "fast_stats_tests.rs"]
mod tests;

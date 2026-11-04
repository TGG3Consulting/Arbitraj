//! Файл: core/reject_log.rs
//! Что делает: решение 155 — журнал повторяющейся причины: одна строка на смену «вида причины» со счётчиком повторов.
//! Живой журнал 04.11: ~11 записей в секунду core.candidate_rejected с одной и той же причиной по направлению (числа
//! прибыли меняются каждый расчёт, поэтому прежнее сравнение полного текста не спасало). Вид причины — текст без чисел
//! (reject_shape.rs: каждое число → «#»), как в решении 139 причину сравнивают по виду, а не по числам. Пока вид тот же — не новая
//! строка, а обновление прежней: счётчик, время последнего повтора, текст и данные — последние (числа последнего
//! расчёта). Строка обновляется не чаще раза в journal.repeat_flush_ms (0 — на каждый повтор); пропущенные обновления
//! дописывает отложенная задача (одна на ключ), чтобы итог не терялся, если повторы прекратились. Смена вида причины или
//! close (кандидат прошёл) — прежняя строка закрывается итоговым счётчиком.
//! journal.repeat_flush_ms не задана — пишем как раньше: новая обычная строка на каждый отказ с другим текстом
//! (данные не скрываются, пока Тигран не выбрал частоту).
//! Кто вызывает: core/candidates.rs (core.candidate_rejected; ключ — направление), тесты.
//! Вид причины считает вызывающий (core/reject_shape.rs).
//! От чего зависит: storage/journal.rs, storage/journal_repeat.rs, tokio (отложенное обновление).
//! Настройки: journal.repeat_flush_ms — читает вызывающий и передаёт сюда (flush).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};

use crate::storage::journal::{Journal, Level};

/// Что записать. kind/level/venue — как у обычной записи; exact — полный ключ сравнения прежнего режима (без
/// journal.repeat_flush_ms); shape — вид причины (без чисел); message, data — текст и данные этого повтора.
pub struct Note<'a> {
    pub level: Level,
    pub kind: &'a str,
    pub venue: Option<&'a str>,
    pub exact: String,
    pub shape: String,
    pub message: String,
    pub data: Option<serde_json::Value>,
}

/// Открытая строка по ключу. id = 0 — строка прежнего режима (обычная запись, без счётчика).
struct Run {
    exact: String,
    shape: String,
    id: i64,
    count: i64,
    last_at: DateTime<Utc>,
    message: String,
    data: Option<serde_json::Value>,
    /// Когда строка в базе обновлялась последний раз.
    flushed: Instant,
    /// Есть повторы, не записанные в базу.
    dirty: bool,
    /// Отложенное обновление уже ждёт.
    scheduled: bool,
}

/// Снимок для обновления строки в базе: (id, счётчик, время последнего, текст, данные).
type Snap = (i64, i64, DateTime<Utc>, String, Option<serde_json::Value>);

impl Run {
    /// Снимок для записи и отметка «записано». Ошибок нет.
    fn take(&mut self) -> Snap {
        self.dirty = false;
        self.flushed = Instant::now();
        (
            self.id,
            self.count,
            self.last_at,
            self.message.clone(),
            self.data.clone(),
        )
    }
}

/// Память открытых строк по ключу (направление). Дешёво клонируется.
#[derive(Clone, Default)]
pub struct RejectLog {
    m: Arc<Mutex<HashMap<String, Run>>>,
}

/// Новая строка — и в лог процесса, как у Deps::log (повторы туда не пишутся). Ошибок нет.
fn trace(n: &Note<'_>) {
    let (kind, venue, msg) = (n.kind, n.venue, &n.message);
    match n.level {
        Level::Error => tracing::error!(kind, venue, "{msg}"),
        Level::Warn => tracing::warn!(kind, venue, "{msg}"),
        Level::Info => tracing::info!(kind, venue, "{msg}"),
    }
}

/// Обновляет строку в базе по снимку (ошибка уже в логе процесса). Ошибок нет.
async fn flush(journal: &Journal, s: Snap) {
    let _ = journal.update_counted(s.0, s.1, s.2, s.3, s.4).await;
}

impl RejectLog {
    fn lock(&self) -> MutexGuard<'_, HashMap<String, Run>> {
        self.m.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Отметить причину по ключу. flush: None — journal.repeat_flush_ms не задана (прежний режим: новая обычная строка
    /// при другом exact); Some(k) — одна строка на вид причины, обновление не чаще раза в k. Принимает: журнал, ключ,
    /// что записать, частоту. Ошибок нет (ошибки базы — в логе процесса; не записанная строка напишется следующим разом).
    pub async fn note(
        &self,
        journal: &Journal,
        key: &str,
        mut n: Note<'_>,
        flush_every: Option<Duration>,
    ) {
        let Some(every) = flush_every else {
            return self.note_plain(journal, key, n).await;
        };
        let now = Utc::now();
        let (snap, schedule, prev) = {
            let mut m = self.lock();
            match m.get_mut(key) {
                Some(r) if r.id > 0 && r.shape == n.shape => {
                    r.count += 1;
                    r.last_at = now;
                    r.message = std::mem::take(&mut n.message);
                    r.data = n.data.take();
                    r.exact = std::mem::take(&mut n.exact);
                    r.dirty = true;
                    let left = every.saturating_sub(r.flushed.elapsed());
                    if left.is_zero() {
                        (Some(r.take()), None, None)
                    } else if !r.scheduled {
                        r.scheduled = true;
                        (None, Some((r.id, left)), None)
                    } else {
                        return;
                    }
                }
                _ => (None, None, Some(m.remove(key))),
            }
        };
        if let Some(s) = snap {
            return flush(journal, s).await;
        }
        if let Some((id, left)) = schedule {
            return self.schedule(journal, key, id, left);
        }
        // Новый вид причины: прежняя строка закрывается итоговым счётчиком, пишется новая.
        if let Some(mut r) = prev.flatten().filter(|r| r.id > 0 && r.dirty) {
            flush(journal, r.take()).await;
        }
        trace(&n);
        if let Ok(e) = journal
            .write_counted(n.level, n.kind, n.venue, n.message.clone(), n.data.clone())
            .await
        {
            let run = Run {
                exact: n.exact,
                shape: n.shape,
                id: e.id,
                count: 1,
                last_at: e.at,
                message: n.message,
                data: n.data,
                flushed: Instant::now(),
                dirty: false,
                scheduled: false,
            };
            self.lock().insert(key.to_string(), run);
        }
    }

    /// Прежний режим: обычная строка, если exact другой, чем в прошлый раз. Ошибок нет.
    async fn note_plain(&self, journal: &Journal, key: &str, n: Note<'_>) {
        let prev = {
            let mut m = self.lock();
            if m.get(key).is_some_and(|r| r.id == 0 && r.exact == n.exact) {
                return;
            }
            m.remove(key)
        };
        trace(&n);
        // Частоту только что сняли: открытая строка со счётчиком закрывается итогом.
        if let Some(mut r) = prev.filter(|r| r.id > 0 && r.dirty) {
            flush(journal, r.take()).await;
        }
        let run = Run {
            exact: n.exact,
            shape: n.shape,
            id: 0,
            count: 1,
            last_at: Utc::now(),
            message: String::new(),
            data: None,
            flushed: Instant::now(),
            dirty: false,
            scheduled: false,
        };
        self.lock().insert(key.to_string(), run);
        let _ = journal
            .write(n.level, n.kind, n.venue, n.message, n.data)
            .await;
    }

    /// Отложенное обновление строки id через left: дописывает повторы, пришедшие после последней записи. Ошибок нет.
    fn schedule(&self, journal: &Journal, key: &str, id: i64, left: Duration) {
        let (me, journal, key) = (self.clone(), journal.clone(), key.to_string());
        tokio::spawn(async move {
            tokio::time::sleep(left).await;
            let snap = {
                let mut m = me.lock();
                match m.get_mut(&key) {
                    Some(r) if r.id == id => {
                        r.scheduled = false;
                        r.dirty.then(|| r.take())
                    }
                    _ => None,
                }
            };
            if let Some(s) = snap {
                flush(&journal, s).await;
            }
        });
    }

    /// Закрыть строку по ключу (кандидат прошёл): итоговый счётчик — в базу, память стирается, следующий отказ —
    /// новая строка. Принимает: журнал, ключ. Ошибок нет.
    pub async fn close(&self, journal: &Journal, key: &str) {
        let prev = self.lock().remove(key);
        if let Some(mut r) = prev.filter(|r| r.id > 0 && r.dirty) {
            flush(journal, r.take()).await;
        }
    }
}

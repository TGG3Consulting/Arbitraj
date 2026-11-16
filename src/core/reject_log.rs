//! Файл: core/reject_log.rs
//! Что делает: решение 155 — журнал повторяющейся причины: одна строка на смену «вида причины» со счётчиком повторов.
//! Живой журнал 04.11: ~11 записей в секунду core.candidate_rejected с одной и той же причиной по направлению (числа
//! прибыли меняются каждый расчёт, поэтому прежнее сравнение полного текста не спасало). Вид причины — текст без чисел
//! (reject_shape.rs: каждое число → «#»), как в решении 139 причину сравнивают по виду, а не по числам. Пока вид тот же —
//! не новая строка, а обновление прежней: счётчик, время последнего повтора, текст и данные — последние (числа
//! последнего расчёта). Строка обновляется не чаще раза в journal.repeat_flush_ms (0 — на каждый повтор); пропущенные
//! обновления дописывает отложенная задача (одна на ключ), чтобы итог не терялся, если повторы прекратились. Смена вида
//! причины или close (кандидат прошёл) — прежняя строка закрывается итоговым счётчиком.
//! journal.repeat_flush_ms не задана — пишем как раньше: новая обычная строка на каждый отказ с другим текстом
//! (данные не скрываются, пока Тигран не выбрал частоту).
//! Решение 354 (по решению владельца — всегда): строки и их обновления не ждут основную базу — новая строка получает
//! метку в памяти, вставка, обновления и «забыть метку» ставятся в очередь журнала синхронно под замком памяти строк
//! (reject_log_q.rs, storage/journal_counted.rs); порядок операций одной строки держит очередь.
//! Кто вызывает: core/candidates.rs (core.candidate_rejected; ключ — направление), core/executor.rs (core.precheck),
//! тесты. Вид причины считает вызывающий (core/reject_shape.rs).
//! От чего зависит: storage/journal.rs, storage/journal_counted.rs, tokio (отложенное обновление).
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

/// Открытая строка по ключу. id = 0 — строка прежнего режима (обычная запись, без счётчика); иначе — метка очереди
/// журнала (решение 354).
pub(super) struct Run {
    pub(super) exact: String,
    pub(super) shape: String,
    pub(super) id: u64,
    pub(super) count: i64,
    pub(super) last_at: DateTime<Utc>,
    pub(super) message: String,
    pub(super) data: Option<serde_json::Value>,
    /// Когда строка в базе обновлялась последний раз.
    pub(super) flushed: Instant,
    /// Есть повторы, не записанные в базу.
    pub(super) dirty: bool,
    /// Отложенное обновление уже ждёт.
    pub(super) scheduled: bool,
}

impl Run {
    /// Обновление строки в очередь журнала и отметка «записано». Принимает: журнал. Ошибок нет.
    pub(super) fn flush(&mut self, journal: &Journal) {
        self.dirty = false;
        self.flushed = Instant::now();
        journal.update_queued(
            self.id,
            self.count,
            self.last_at,
            self.message.clone(),
            self.data.clone(),
        );
    }

    /// Строка уходит из памяти: итог (если есть незаписанные повторы) и «забыть метку». Принимает: журнал.
    /// Ошибок нет.
    pub(super) fn close(&mut self, journal: &Journal) {
        if self.id == 0 {
            return;
        }
        if self.dirty {
            self.flush(journal);
        }
        journal.forget_queued(self.id);
    }
}

/// Память открытых строк по ключу (направление). Дешёво клонируется.
#[derive(Clone, Default)]
pub struct RejectLog {
    m: Arc<Mutex<HashMap<String, Run>>>,
}

/// Новая строка — и в лог процесса, как у Deps::log (повторы туда не пишутся). Ошибок нет.
pub(super) fn trace(n: &Note<'_>) {
    let (kind, venue, msg) = (n.kind, n.venue, &n.message);
    match n.level {
        Level::Error => tracing::error!(kind, venue, "{msg}"),
        Level::Warn => tracing::warn!(kind, venue, "{msg}"),
        Level::Info => tracing::info!(kind, venue, "{msg}"),
    }
}

impl RejectLog {
    pub(super) fn lock(&self) -> MutexGuard<'_, HashMap<String, Run>> {
        self.m.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Отметить причину по ключу. flush: None — journal.repeat_flush_ms не задана (прежний режим: новая обычная строка
    /// при другом exact); Some(k) — одна строка на вид причины, обновление не чаще раза в k. Принимает: журнал, ключ,
    /// что записать, частоту. Ошибок нет; ничего не ждёт (решение 354).
    pub async fn note(
        &self,
        journal: &Journal,
        key: &str,
        n: Note<'_>,
        flush_every: Option<Duration>,
    ) {
        match flush_every {
            Some(every) => self.note_queued(journal, key, n, every),
            None => self.note_plain(journal, key, n).await,
        }
    }

    /// Прежний режим: обычная строка, если exact другой, чем в прошлый раз. Ошибок нет.
    async fn note_plain(&self, journal: &Journal, key: &str, n: Note<'_>) {
        {
            let mut m = self.lock();
            if m.get(key).is_some_and(|r| r.id == 0 && r.exact == n.exact) {
                return;
            }
            // Частоту только что сняли: открытая строка со счётчиком закрывается итогом.
            if let Some(mut r) = m.remove(key) {
                r.close(journal);
            }
            let run = Run {
                exact: n.exact.clone(),
                shape: n.shape.clone(),
                id: 0,
                count: 1,
                last_at: Utc::now(),
                message: String::new(),
                data: None,
                flushed: Instant::now(),
                dirty: false,
                scheduled: false,
            };
            m.insert(key.to_string(), run);
        }
        trace(&n);
        let _ = journal
            .write(n.level, n.kind, n.venue, n.message, n.data)
            .await;
    }

    /// Отложенное обновление строки id через left: дописывает повторы, пришедшие после последней записи (в очередь —
    /// под замком памяти строк, порядок с закрытием). Ошибок нет.
    pub(super) fn schedule(&self, journal: &Journal, key: &str, id: u64, left: Duration) {
        let (me, journal, key) = (self.clone(), journal.clone(), key.to_string());
        tokio::spawn(async move {
            tokio::time::sleep(left).await;
            let mut m = me.lock();
            if let Some(r) = m.get_mut(&key)
                && r.id == id
            {
                r.scheduled = false;
                if r.dirty {
                    r.flush(&journal);
                }
            }
        });
    }

    /// Закрыть строку по ключу (кандидат прошёл): итоговый счётчик — в очередь журнала, память стирается, следующий
    /// отказ — новая строка. Принимает: журнал, ключ. Ошибок нет; ничего не ждёт (решение 354).
    pub async fn close(&self, journal: &Journal, key: &str) {
        if let Some(mut r) = self.lock().remove(key) {
            r.close(journal);
        }
    }
}

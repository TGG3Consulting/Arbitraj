//! Файл: core/reject_log_q.rs
//! Что делает: решение 354 (задача t158, Тигран 16.11: «ничего не должно тормозить сделку нигде») — журнал
//! повторяющейся причины (reject_log.rs, решение 155) при включённой очереди журнала (journal.queue_max): отказ
//! предпроверки (executor.rs) и отказ кандидата (candidates.rs) не ждут основную базу ради id строки. Новая строка
//! получает метку в памяти (Journal::counted_queued), обновления и закрытие — по метке через ту же очередь; всё
//! ставится синхронно под замком памяти строк — порядок «строка → обновления → забыть метку» один для всех задач
//! (отложенное обновление schedule и close делают то же под тем же замком). Логика строк (вид причины, счётчик, частота
//! journal.repeat_flush_ms) — та же, что у прямого пути.
//! Кто вызывает: core/reject_log.rs (RejectLog::note).
//! От чего зависит: core/reject_log.rs (Run, Note, trace, enqueue, forget, schedule), storage/journal_counted.rs.
//! Настройки: journal.repeat_flush_ms (частота — передаёт вызывающий), journal.queue_max (включает этот путь).

use std::time::{Duration, Instant};

use chrono::Utc;

use crate::storage::journal::Journal;

use super::reject_log::{Note, RejectLog, Run, enqueue, forget, trace};

impl RejectLog {
    /// Что делает: отметить причину по ключу без ожидания базы (решение 354). Принимает: журнал, ключ, что записать,
    /// частоту обновления строки. Ошибок нет (очередь полна — строка не записана, потеря посчитана журналом; следующий
    /// отказ попробует снова).
    pub(super) fn note_queued(
        &self,
        journal: &Journal,
        key: &str,
        mut n: Note<'_>,
        every: Duration,
    ) {
        let now = Utc::now();
        let mut m = self.lock();
        if let Some(r) = m.get_mut(key)
            && r.id > 0
            && r.shape == n.shape
        {
            r.count += 1;
            r.last_at = now;
            r.message = std::mem::take(&mut n.message);
            r.data = n.data.take();
            r.exact = std::mem::take(&mut n.exact);
            r.dirty = true;
            let left = every.saturating_sub(r.flushed.elapsed());
            if left.is_zero() {
                let token = r.token;
                let s = r.take();
                enqueue(journal, s, token);
            } else if !r.scheduled {
                r.scheduled = true;
                let row = (r.id, r.token);
                drop(m);
                self.schedule(journal, key, row, left);
            }
            return;
        }
        // Новый вид причины (или первый отказ): прежняя строка закрывается итоговым счётчиком, пишется новая.
        if let Some(mut r) = m.remove(key) {
            if r.id > 0 && r.dirty {
                let token = r.token;
                let s = r.take();
                enqueue(journal, s, token);
            }
            forget(journal, &r);
        }
        trace(&n);
        let Some(token) =
            journal.counted_queued(n.level, n.kind, n.venue, n.message.clone(), n.data.clone())
        else {
            return;
        };
        let run = Run {
            exact: n.exact,
            shape: n.shape,
            id: token as i64,
            token: true,
            count: 1,
            last_at: now,
            message: n.message,
            data: n.data,
            flushed: Instant::now(),
            dirty: false,
            scheduled: false,
        };
        m.insert(key.to_string(), run);
    }
}

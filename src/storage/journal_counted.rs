//! Файл: storage/journal_counted.rs
//! Что делает: решение 354 (задача t158) — повторяющиеся строки журнала (решение 155) без ожидания основной базы:
//! новая строка получает метку в памяти (Queue::next_token) и уходит в очередь записи журнала; обновления и «забыть» —
//! той же очередью по метке (или по настоящему id, если строка записана прямо, до включения очереди). Вызывающий не
//! ждёт ничего: постановка — синхронная, её можно делать под своим замком, тогда порядок операций одной строки
//! задаёт сам вызывающий. Работает при включённой очереди журнала (journal.queue_max); операции по метке ставятся и
//! после выключения (карту меток знает только поток-писатель, он живёт, пока жив журнал).
//! Кто вызывает: core/reject_log.rs (строки отказов кандидатов и предпроверки).
//! От чего зависит: storage/journal.rs (Journal, Entry, Level), storage/journal_queue.rs (push_op, next_token),
//! storage/journal_op.rs (Op, Upd, Target).
//! Настройки: journal.queue_max (включает путь меток; читает storage/write_modes.rs).

use chrono::{DateTime, Utc};

use super::journal::{Entry, Journal, Level};
use super::journal_op::{Op, Target, Upd};
use super::journal_queue::Pushed;

impl Journal {
    /// Что делает: новая повторяющаяся строка (счётчик 1) через очередь — без ожидания базы. Принимает: уровень, вид,
    /// площадку, текст, данные. Возвращает: метку строки (для обновлений) или None — очередь полна (потеря посчитана).
    /// Ошибок нет.
    pub fn counted_queued(
        &self,
        level: Level,
        kind: &str,
        venue: Option<&str>,
        message: String,
        data: Option<serde_json::Value>,
    ) -> Option<u64> {
        let at = Utc::now();
        let e = Entry {
            id: 0,
            at,
            level,
            kind: kind.to_string(),
            venue: venue.map(str::to_string),
            message,
            data,
            repeats: Some(1),
            last_at: Some(at),
        };
        let token = self.queue.next_token();
        match self
            .queue
            .push_op(&self.db, &self.live, Op::Counted(token, Box::new(e)), true)
        {
            Pushed::Queued => Some(token),
            Pushed::Lost | Pushed::Off(_) => None,
        }
    }

    /// Что делает: обновление повторяющейся строки через очередь (как update_counted, без ожидания). Принимает: цель
    /// (метка или id), счётчик, время последнего, текст, данные. Ошибок нет (не записалось — лог процесса писателя).
    pub fn update_queued(
        &self,
        target: Target,
        repeats: i64,
        last_at: DateTime<Utc>,
        message: String,
        data: Option<serde_json::Value>,
    ) {
        let u = Upd {
            target,
            repeats,
            last_at,
            message,
            data,
        };
        let _ = self
            .queue
            .push_op(&self.db, &self.live, Op::Update(Box::new(u)), true);
    }

    /// Что делает: метка строки больше не нужна (строка закрыта) — писатель убирает её из карты. Принимает: метку.
    /// Ошибок нет.
    pub fn forget_queued(&self, token: u64) {
        let _ = self
            .queue
            .push_op(&self.db, &self.live, Op::Forget(token), true);
    }
}

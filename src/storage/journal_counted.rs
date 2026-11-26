//! Файл: storage/journal_counted.rs
//! Что делает: решение 354 (задача t158) — повторяющиеся строки журнала (решение 155) без ожидания основной базы:
//! новая строка получает метку в памяти (Queue::next_token) и уходит в очередь записи журнала; обновления и «забыть» —
//! той же очередью по метке. Вызывающий не ждёт ничего: постановка синхронная, её можно делать под своим замком —
//! тогда порядок операций одной строки задаёт сам вызывающий.
//! Кто вызывает: core/reject_log.rs, core/reject_log_q.rs (строки отказов кандидатов и предпроверки).
//! От чего зависит: storage/journal.rs (Journal, Entry, Level), storage/db.rs (очередь журнала базы),
//! storage/journal_op.rs (Op, Row, Upd).
//! Настройки: journal.queue_max (предел длины очереди; читает storage/write_modes.rs).

use chrono::{DateTime, Utc};

use super::journal::{Entry, Journal, Level};
use super::journal_op::{Op, Row, Upd};

impl Journal {
    /// Что делает: новая повторяющаяся строка (счётчик 1) через очередь — без ожидания базы. Принимает: уровень, вид,
    /// площадку, текст, данные. Возвращает: метку строки (для обновлений) или None — очередь полна (потеря
    /// посчитана). Ошибок нет.
    pub fn counted_queued(
        &self,
        level: Level,
        kind: &str,
        venue: Option<&str>,
        message: String,
        data: Option<serde_json::Value>,
    ) -> Option<u64> {
        self.counted_queued_with_repeats(level, kind, venue, message, data, 1)
    }

    /// Восстановленная сводка может уже содержать несколько событий до первой успешной постановки.
    pub(crate) fn counted_queued_with_repeats(
        &self,
        level: Level,
        kind: &str,
        venue: Option<&str>,
        message: String,
        data: Option<serde_json::Value>,
        repeats: i64,
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
            repeats: Some(repeats),
            last_at: Some(at),
        };
        let q = self.db.journal_queue();
        let token = q.next_token();
        let row = Row {
            e,
            live: Some(self.live.clone()),
        };
        q.push_op(self.db.conn_arc(), Op::Counted(token, Box::new(row)))
            .then_some(token)
    }

    /// Что делает: обновление повторяющейся строки по метке через очередь (счётчик только растёт). Принимает: метку,
    /// счётчик, время последнего, текст, данные. Ошибок нет (не записалось — лог процесса и запись о потерях).
    pub fn update_queued(
        &self,
        token: u64,
        repeats: i64,
        last_at: DateTime<Utc>,
        message: String,
        data: Option<serde_json::Value>,
    ) {
        let u = Upd {
            token,
            repeats,
            last_at,
            message,
            data,
            live: self.live.clone(),
        };
        let _ = self
            .db
            .journal_queue()
            .push_op(self.db.conn_arc(), Op::Update(Box::new(u)));
    }

    /// Что делает: метка строки больше не нужна (строка закрыта) — писатель убирает её из карты. Принимает: метку.
    /// Ошибок нет.
    pub fn forget_queued(&self, token: u64) {
        let _ = self
            .db
            .journal_queue()
            .push_op(self.db.conn_arc(), Op::Forget(token));
    }
}

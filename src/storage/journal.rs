//! Файл: storage/journal.rs
//! Что делает: журнал системы в SQLite — ошибки, уведомления, изменения настроек, события.
//! Каждая запись также уходит в живой поток для интерфейса (broadcast).
//! Решение 155: запись может быть «повторяющейся» — одна строка на смену причины со счётчиком повторов (`repeats`) и
//! временем последнего (`last_at`); такие строки ставит storage/journal_counted.rs. У обычных записей оба пусты.
//! Решение 341 (по решению владельца 16.11 — всегда, без выключателя): запись не ждётся — уходит в очередь журнала базы,
//! отдельный поток ОС пишет накопленное пачками по порядку (storage/journal_queue.rs, journal_writer.rs); чтение
//! (list) дожидается записи всего, что поставлено до него.
//! Кто вызывает: settings/store.rs (изменения настроек), notify/, api/, ядро и адаптеры (через Deps::log, ctx.log).
//! От чего зависит: storage/db.rs (очередь журнала базы), storage/journal_op.rs, tokio::sync::broadcast.
//! Настройки: journal.queue_max (предел длины очереди), journal.batch_max (размер пачки) — читает
//! storage/write_modes.rs и передаёт set_queue_max, set_batch_max.

use chrono::{DateTime, Utc};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

use super::db::{Db, DbError};
use super::journal_op::{Op, Row};

/// Уровень записи.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Level {
    Info,
    Warn,
    Error,
}

/// Запись журнала.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    /// Номер записи; 0 — ещё не сохранена.
    pub id: i64,
    pub at: DateTime<Utc>,
    pub level: Level,
    /// Вид события (например «settings.changed», «notify.sent», «gateway.error»).
    pub kind: String,
    pub venue: Option<String>,
    /// Понятный текст по-русски.
    pub message: String,
    /// Дополнительные данные (без секретов).
    pub data: Option<serde_json::Value>,
    /// Решение 155: сколько раз повторилась та же причина (`at` — первый раз, `last_at` — последний). Нет — обычная
    /// запись.
    #[serde(default)]
    pub repeats: Option<i64>,
    /// Решение 155: время последнего повтора; нет — обычная запись.
    #[serde(default)]
    pub last_at: Option<DateTime<Utc>>,
}

/// Журнал: запись в базу + живой поток.
#[derive(Clone)]
pub struct Journal {
    pub(super) db: Db,
    pub(super) live: broadcast::Sender<Entry>,
}

impl Journal {
    /// Создаёт журнал.
    /// Принимает: базу и отправителя живого потока (его же слушает api/ws).
    pub fn new(db: Db, live: broadcast::Sender<Entry>) -> Self {
        Self { db, live }
    }

    /// База журнала — для компонентов, которым нужно своё хранение (например, очередь кошелька DEX). Ошибок нет.
    pub fn db(&self) -> &Db {
        &self.db
    }

    /// Пишет запись. Решение 341 (по решению владельца — всегда): запись уходит в очередь журнала базы
    /// (storage/journal_queue.rs) и не ждётся — ни одна запись журнала не задерживает сделку и расчёт; поток-писатель
    /// пишет её пачкой и отдаёт в живой поток с настоящим id. Принимает: уровень, вид, площадку, текст, данные.
    /// Возвращает: запись с id = 0 (результат write в рабочем коде не читает никто — разбор t158, таблица 1).
    /// Ошибки: нет (очередь полна или пачка не записалась — счёт потерь и запись journal.queue_lost, не молча;
    /// Result оставлен ради прежней сигнатуры у ~90 вызовов).
    pub async fn write(
        &self,
        level: Level,
        kind: &str,
        venue: Option<&str>,
        message: impl Into<String>,
        data: Option<serde_json::Value>,
    ) -> Result<Entry, DbError> {
        Ok(self.enqueue(level, kind, venue, message.into(), data))
    }

    /// Ставит запись в очередь (не-async: временные значения не попадают в будущие вызывающих). Принимает: как write.
    /// Возвращает: запись (id = 0). Ошибок нет.
    fn enqueue(
        &self,
        level: Level,
        kind: &str,
        venue: Option<&str>,
        message: String,
        data: Option<serde_json::Value>,
    ) -> Entry {
        let e = Entry {
            id: 0,
            at: Utc::now(),
            level,
            kind: kind.to_string(),
            venue: venue.map(str::to_string),
            message,
            data,
            repeats: None,
            last_at: None,
        };
        let row = Row {
            e: e.clone(),
            live: Some(self.live.clone()),
        };
        let _ = self
            .db
            .journal_queue()
            .push_op(self.db.conn_arc(), Op::Insert(Box::new(row)));
        e
    }

    /// Решение 341: предел длины очереди записи (journal.queue_max); None или 0 — без предела. Ошибок нет.
    pub fn set_queue_max(&self, max: Option<u64>) {
        self.db.journal_queue().set_max(max);
    }

    /// Решение 341: предел записей в одной транзакции пачки (journal.batch_max); None или 0 — всё накопленное.
    /// Ошибок нет.
    pub fn set_batch_max(&self, max: Option<u64>) {
        self.db.journal_queue().set_batch_max(max);
    }

    /// Решение 341: ждёт, пока очередь записи журнала опустеет (всё записано в базу). Ошибок нет.
    pub async fn idle(&self) {
        self.db.journal_queue().idle().await;
    }

    /// Читает записи журнала, новые первыми.
    /// Принимает: сколько записей (задаёт интерфейс) и номер, до которого читать (для листания).
    /// Ошибки: база.
    /// Решение 341: сначала дожидается записи всего, что поставлено в очередь до вызова («записал — прочитал»).
    pub async fn list(&self, limit: u32, before_id: Option<i64>) -> Result<Vec<Entry>, DbError> {
        self.db.journal_queue().flushed().await;
        self.db
            .call(move |c| {
                let mut st = c.prepare(&format!(
                    "SELECT {COLUMNS} FROM journal WHERE (?1 IS NULL OR id < ?1) ORDER BY id DESC LIMIT ?2"
                ))?;
                let rows = st.query_map(params![before_id, limit], row_entry)?;
                rows.collect()
            })
            .await
    }
}

/// Вставляет запись в таблицу journal (без id). Принимает: соединение (или транзакцию), запись. Возвращает: id.
/// Ошибки: SQLite.
pub(super) fn insert(c: &rusqlite::Connection, row: &Entry) -> rusqlite::Result<i64> {
    c.execute(
        "INSERT INTO journal (at, level, kind, venue, message, data) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            row.at.to_rfc3339(),
            format!("{:?}", row.level),
            row.kind,
            row.venue,
            row.message,
            row.data.as_ref().map(|d| d.to_string())
        ],
    )?;
    Ok(c.last_insert_rowid())
}

/// Столбцы записи для row_entry — в этом порядке (list и journal_op.rs).
pub(super) const COLUMNS: &str = "id, at, level, kind, venue, message, data, repeats, last_at";

/// Время из текста базы (RFC 3339). Принимает: номер столбца, текст. Ошибки: не время.
fn time(col: usize, s: &str) -> rusqlite::Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .map(|t| t.with_timezone(&Utc))
        .map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(col, rusqlite::types::Type::Text, Box::new(e))
        })
}

/// Строка базы (столбцы COLUMNS) → запись. Ошибки: неизвестный уровень, не время.
pub(super) fn row_entry(r: &rusqlite::Row<'_>) -> rusqlite::Result<Entry> {
    let at: String = r.get(1)?;
    let level: String = r.get(2)?;
    let data: Option<String> = r.get(6)?;
    let last_at: Option<String> = r.get(8)?;
    Ok(Entry {
        id: r.get(0)?,
        at: time(1, &at)?,
        level: match level.as_str() {
            "Error" => Level::Error,
            "Warn" => Level::Warn,
            "Info" => Level::Info,
            other => {
                return Err(rusqlite::Error::InvalidColumnType(
                    2,
                    other.to_string(),
                    rusqlite::types::Type::Text,
                ));
            }
        },
        kind: r.get(3)?,
        venue: r.get(4)?,
        message: r.get(5)?,
        data: data.and_then(|d| serde_json::from_str(&d).ok()),
        repeats: r.get(7)?,
        last_at: last_at.as_deref().map(|t| time(8, t)).transpose()?,
    })
}

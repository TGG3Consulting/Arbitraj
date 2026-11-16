//! Файл: storage/journal.rs
//! Что делает: журнал системы в SQLite — ошибки, уведомления, изменения настроек, события.
//! Каждая запись также уходит в живой поток для интерфейса (broadcast).
//! Решение 155: запись может быть «повторяющейся» — одна строка на смену причины со счётчиком повторов (`repeats`) и
//! временем последнего (`last_at`); запись и обновление таких строк — journal_repeat.rs. У обычных записей оба пусты.
//! Решение 341: при заданной journal.queue_max запись не ждётся — она уходит в очередь, отдельный поток ОС пишет
//! накопленное одной транзакцией по порядку (storage/journal_queue.rs); не задана — как раньше (запись ждётся).
//! Кто вызывает: settings/store.rs (изменения настроек), notify/, api/; позже шлюз (ошибки).
//! От чего зависит: storage/db.rs, storage/journal_queue.rs, tokio::sync::broadcast.
//! Настройки: journal.queue_max (включает очередь), journal.batch_max (размер пачки) — читает storage/write_modes.rs и
//! передаёт set_queue_max, set_batch_max.

use chrono::{DateTime, Utc};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

use super::db::{Db, DbError};
use super::journal_queue::Queue;

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
    /// Решение 341: очередь записи (выключена, пока не задана journal.queue_max).
    pub(super) queue: std::sync::Arc<Queue>,
}

impl Journal {
    /// Создаёт журнал.
    /// Принимает: базу и отправителя живого потока (его же слушает api/ws).
    pub fn new(db: Db, live: broadcast::Sender<Entry>) -> Self {
        Self {
            db,
            live,
            queue: std::sync::Arc::new(Queue::default()),
        }
    }

    /// База журнала — для компонентов, которым нужно своё хранение (например, очередь кошелька DEX). Ошибок нет.
    pub fn db(&self) -> &Db {
        &self.db
    }

    /// Пишет запись. Ошибки базы не теряются: выводятся в лог процесса и возвращаются.
    /// Принимает: уровень, вид, площадку, текст, данные. Возвращает: сохранённую запись.
    /// Решение 341: очередь включена (journal.queue_max) — запись уходит в очередь и не ждётся; возвращается запись с
    /// id = 0 (настоящий id — в живом потоке после записи пачки; результат write никто в коде не читает — разбор t158);
    /// очередь полна — запись потеряна со счётом (запись journal.queue_lost пишет поток очереди).
    pub async fn write(
        &self,
        level: Level,
        kind: &str,
        venue: Option<&str>,
        message: impl Into<String>,
        data: Option<serde_json::Value>,
    ) -> Result<Entry, DbError> {
        let mut e = Entry {
            id: 0,
            at: Utc::now(),
            level,
            kind: kind.to_string(),
            venue: venue.map(str::to_string),
            message: message.into(),
            data,
            repeats: None,
            last_at: None,
        };
        let row = e.clone();
        let Some(row) = self.queue.push(&self.db, &self.live, row) else {
            return Ok(e);
        };
        let res = self.db.call(move |c| insert(c, &row)).await;
        match res {
            Ok(id) => {
                e.id = id;
                // Нет слушателей — не ошибка: интерфейс может быть закрыт.
                let _ = self.live.send(e.clone());
                Ok(e)
            }
            Err(err) => {
                tracing::error!(%err, kind = %e.kind, "не удалось записать в журнал");
                Err(err)
            }
        }
    }

    /// Решение 341: включена ли очередь записи (или в ней ещё есть записи) — запись журнала сейчас не ждётся.
    /// Ошибок нет.
    pub fn queued(&self) -> bool {
        self.queue.on()
    }

    /// Решение 341: предел очереди записи (journal.queue_max); None или 0 — очередь выключена (как раньше). Ошибок нет.
    pub fn set_queue_max(&self, max: Option<u64>) {
        self.queue.set_max(max);
    }

    /// Решение 341: предел записей в одной транзакции пачки (journal.batch_max); None или 0 — всё накопленное.
    /// Ошибок нет.
    pub fn set_batch_max(&self, max: Option<u64>) {
        self.queue.set_batch_max(max);
    }

    /// Решение 341: ждёт, пока очередь записи опустеет (всё записано в базу). Очереди нет — сразу. Ошибок нет.
    pub async fn idle(&self) {
        self.queue.idle().await;
    }

    /// Читает записи журнала, новые первыми.
    /// Принимает: сколько записей (задаёт интерфейс) и номер, до которого читать (для листания).
    /// Ошибки: база.
    pub async fn list(&self, limit: u32, before_id: Option<i64>) -> Result<Vec<Entry>, DbError> {
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

/// Столбцы записи для row_entry — в этом порядке (list и journal_repeat.rs).
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

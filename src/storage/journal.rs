//! Файл: storage/journal.rs
//! Что делает: журнал системы в SQLite — ошибки, уведомления, изменения настроек, события.
//! Каждая запись также уходит в живой поток для интерфейса (broadcast).
//! Кто вызывает: settings/store.rs, notify/, api/, шлюз (ошибки).
//! От чего зависит: storage/db.rs, tokio::sync::broadcast.
//! Настройки: нет.

use chrono::{DateTime, Utc};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

use super::db::{Db, DbError};

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
}

/// Журнал: запись в базу + живой поток.
#[derive(Clone)]
pub struct Journal {
    db: Db,
    live: broadcast::Sender<Entry>,
}

impl Journal {
    /// Создаёт журнал.
    /// Принимает: базу и отправителя живого потока (его же слушает api/ws).
    pub fn new(db: Db, live: broadcast::Sender<Entry>) -> Self {
        Self { db, live }
    }

    /// Пишет запись. Ошибки базы не теряются: выводятся в лог процесса и возвращаются.
    /// Принимает: уровень, вид, площадку, текст, данные. Возвращает: сохранённую запись.
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
        };
        let row = e.clone();
        let res = self
            .db
            .call(move |c| {
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
            })
            .await;
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

    /// Читает записи журнала, новые первыми.
    /// Принимает: сколько записей (задаёт интерфейс) и номер, до которого читать (для листания).
    /// Ошибки: база.
    pub async fn list(&self, limit: u32, before_id: Option<i64>) -> Result<Vec<Entry>, DbError> {
        self.db
            .call(move |c| {
                let mut st = c.prepare(
                    "SELECT id, at, level, kind, venue, message, data FROM journal
                     WHERE (?1 IS NULL OR id < ?1) ORDER BY id DESC LIMIT ?2",
                )?;
                let rows = st.query_map(params![before_id, limit], |r| {
                    let at: String = r.get(1)?;
                    let level: String = r.get(2)?;
                    let data: Option<String> = r.get(6)?;
                    Ok(Entry {
                        id: r.get(0)?,
                        at: DateTime::parse_from_rfc3339(&at)
                            .map(|t| t.with_timezone(&Utc))
                            .map_err(|e| {
                                rusqlite::Error::FromSqlConversionFailure(
                                    1,
                                    rusqlite::types::Type::Text,
                                    Box::new(e),
                                )
                            })?,
                        level: match level.as_str() {
                            "Error" => Level::Error,
                            "Warn" => Level::Warn,
                            _ => Level::Info,
                        },
                        kind: r.get(3)?,
                        venue: r.get(4)?,
                        message: r.get(5)?,
                        data: data.and_then(|d| serde_json::from_str(&d).ok()),
                    })
                })?;
                rows.collect()
            })
            .await
    }
}

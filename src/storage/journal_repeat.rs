//! Файл: storage/journal_repeat.rs
//! Что делает: решение 155 — «повторяющиеся» записи журнала: одна строка на смену причины со счётчиком повторов.
//! Новая строка пишется со счётчиком 1 (`repeats` = 1, `last_at` = `at`); повторы той же причины обновляют ту же строку:
//! счётчик, время последнего повтора, текст и данные — последние. Обновлённая запись снова уходит в живой поток с тем же
//! id — экран «Журнал» заменяет строку, а не добавляет новую.
//! Кто вызывает: core/reject_log.rs (core.candidate_rejected).
//! От чего зависит: storage/journal.rs (Journal, Entry, row_entry), storage/db.rs.
//! Настройки: нет (как часто обновлять строку — journal.repeat_flush_ms, читает вызывающий).

use chrono::{DateTime, Utc};
use rusqlite::{OptionalExtension, params};

use super::db::DbError;
use super::journal::{COLUMNS, Entry, Journal, Level, row_entry};

impl Journal {
    /// Пишет новую повторяющуюся запись: счётчик 1, время последнего = время записи. Уходит в живой поток.
    /// Принимает: уровень, вид, площадку, текст, данные. Возвращает: сохранённую запись (её id — для обновлений).
    /// Ошибки: база (уже выведена в лог процесса).
    pub async fn write_counted(
        &self,
        level: Level,
        kind: &str,
        venue: Option<&str>,
        message: impl Into<String>,
        data: Option<serde_json::Value>,
    ) -> Result<Entry, DbError> {
        let at = Utc::now();
        let mut e = Entry {
            id: 0,
            at,
            level,
            kind: kind.to_string(),
            venue: venue.map(str::to_string),
            message: message.into(),
            data,
            repeats: Some(1),
            last_at: Some(at),
        };
        let row = e.clone();
        let res = self
            .db
            .call(move |c| {
                c.execute(
                    "INSERT INTO journal (at, level, kind, venue, message, data, repeats, last_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1, ?1)",
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
                let _ = self.live.send(e.clone());
                Ok(e)
            }
            Err(err) => {
                tracing::error!(%err, kind = %e.kind, "не удалось записать в журнал");
                Err(err)
            }
        }
    }

    /// Обновляет повторяющуюся запись: счётчик, время последнего повтора, текст и данные — последние. Счётчик только
    /// растёт: обновление со счётчиком меньше записанного (опоздавшее — гонка отложенного обновления с закрытием)
    /// ничего не меняет. Обновлённая запись уходит в живой поток с тем же id.
    /// Принимает: id записи, счётчик, время последнего, текст, данные. Возвращает: запись после обновления (None —
    /// записи нет или обновление опоздало). Ошибки: база (уже выведена в лог процесса).
    pub async fn update_counted(
        &self,
        id: i64,
        repeats: i64,
        last_at: DateTime<Utc>,
        message: String,
        data: Option<serde_json::Value>,
    ) -> Result<Option<Entry>, DbError> {
        let res = self
            .db
            .call(move |c| {
                let n = c.execute(
                    "UPDATE journal SET repeats = ?2, last_at = ?3, message = ?4, data = ?5
                     WHERE id = ?1 AND (repeats IS NULL OR repeats <= ?2)",
                    params![
                        id,
                        repeats,
                        last_at.to_rfc3339(),
                        message,
                        data.as_ref().map(|d| d.to_string())
                    ],
                )?;
                if n == 0 {
                    return Ok(None);
                }
                c.query_row(
                    &format!("SELECT {COLUMNS} FROM journal WHERE id = ?1"),
                    params![id],
                    row_entry,
                )
                .optional()
            })
            .await;
        match res {
            Ok(e) => {
                if let Some(e) = &e {
                    let _ = self.live.send(e.clone());
                }
                Ok(e)
            }
            Err(err) => {
                tracing::error!(%err, id, "не удалось обновить запись журнала");
                Err(err)
            }
        }
    }
}

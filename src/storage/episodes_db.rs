//! Файл: storage/episodes_db.rs
//! Что делает: список эпизодов арбитражной ситуации (решение 74) в SQLite — таблица episodes: id, время начала, JSON
//! сводки (монета, направление, начало, конец, наибольшая прибыль, причина закрытия; тип — core/episode.rs). Сами
//! записи — в файле эпизода (storage/episode_files.rs).
//! Кто вызывает: core/episode_run.rs (сохранение, список, удаление по сроку и месту), core/engine.rs (при запуске — закрыть
//! прерванные перезапуском).
//! От чего зависит: storage/db.rs.
//! Настройки: нет.

use chrono::Utc;
use rusqlite::{OptionalExtension, params};

use super::db::{Db, DbError};

/// Сохраняет сводку (заменяет прежнюю). Принимает: id, начало (RFC 3339), JSON. Ошибки: база.
pub async fn save(db: &Db, id: &str, started_at: String, body: String) -> Result<(), DbError> {
    let id = id.to_string();
    let at = Utc::now().to_rfc3339();
    db.call(move |c| {
        c.execute(
            "INSERT INTO episodes (id, started_at, body, updated_at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(id) DO UPDATE SET body = excluded.body, updated_at = excluded.updated_at",
            params![id, started_at, body, at],
        )
        .map(|_| ())
    })
    .await
}

/// Сводки, новые сверху. Принимает: сколько не больше. Возвращает: JSON сводок. Ошибки: база.
pub async fn list(db: &Db, limit: u32) -> Result<Vec<String>, DbError> {
    db.call(move |c| {
        let mut st = c.prepare("SELECT body FROM episodes ORDER BY started_at DESC LIMIT ?1")?;
        let rows = st.query_map(params![limit], |r| r.get(0))?;
        rows.collect()
    })
    .await
}

/// Все сводки, старые сверху (для удаления по сроку и месту). Ошибки: база.
pub async fn oldest_first(db: &Db) -> Result<Vec<String>, DbError> {
    db.call(|c| {
        let mut st = c.prepare("SELECT body FROM episodes ORDER BY started_at ASC")?;
        let rows = st.query_map([], |r| r.get(0))?;
        rows.collect()
    })
    .await
}

/// Одна сводка. None — нет такого эпизода. Ошибки: база.
pub async fn get(db: &Db, id: &str) -> Result<Option<String>, DbError> {
    let id = id.to_string();
    db.call(move |c| {
        c.query_row(
            "SELECT body FROM episodes WHERE id = ?1",
            params![id],
            |r| r.get(0),
        )
        .optional()
    })
    .await
}

/// Удаляет сводку. Ошибки: база.
pub async fn delete(db: &Db, id: &str) -> Result<(), DbError> {
    let id = id.to_string();
    db.call(move |c| {
        c.execute("DELETE FROM episodes WHERE id = ?1", params![id])
            .map(|_| ())
    })
    .await
}

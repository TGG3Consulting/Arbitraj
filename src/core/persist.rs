//! Файл: core/persist.rs
//! Что делает: учёт ядра в SQLite — сделки, переводы, статистика газа, алерты, ручное управление,
//! итог дня (ТЗ, стек: «Сделки, переводы, учёт — SQLite»). Одна таблица core_state: вид, id, JSON.
//! Таблица создаётся ядром через общий доступ к базе (storage/db.rs, Db::call); схема — здесь,
//! в зоне ядра (перенос в storage/ — вопрос в Issue #8).
//! Кто вызывает: ledger.rs, gas_stats.rs, manual.rs, transfers.rs.
//! От чего зависит: storage/db.rs, serde_json.
//! Настройки: нет.

use chrono::Utc;
use rusqlite::params;
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::storage::db::{Db, DbError};

/// Схема таблицы учёта ядра.
const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS core_state (
    kind TEXT NOT NULL,
    id   TEXT NOT NULL,
    body TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (kind, id)
);";

/// Создаёт таблицу, если её нет. Ошибки: база.
pub async fn init(db: &Db) -> Result<(), DbError> {
    db.call(|c| c.execute_batch(SCHEMA)).await
}

/// Сохраняет запись (новую или поверх старой). Принимает: вид, id, значение. Ошибки: база, JSON.
pub async fn save<T: Serialize>(db: &Db, kind: &str, id: &str, v: &T) -> Result<(), DbError> {
    let body = serde_json::to_string(v).map_err(|e| DbError::Json(e.to_string()))?;
    let (kind, id) = (kind.to_string(), id.to_string());
    db.call(move |c| {
        c.execute(
            "INSERT INTO core_state (kind, id, body, updated_at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(kind, id) DO UPDATE SET body = excluded.body, updated_at = excluded.updated_at",
            params![kind, id, body, Utc::now().to_rfc3339()],
        )
        .map(|_| ())
    })
    .await
}

/// Удаляет запись. Ошибки: база.
pub async fn remove(db: &Db, kind: &str, id: &str) -> Result<(), DbError> {
    let (kind, id) = (kind.to_string(), id.to_string());
    db.call(move |c| {
        c.execute(
            "DELETE FROM core_state WHERE kind = ?1 AND id = ?2",
            params![kind, id],
        )
        .map(|_| ())
    })
    .await
}

/// Все записи вида. Повреждённые строки пропускаются с записью в лог процесса.
/// Ошибки: база.
pub async fn load<T: DeserializeOwned + Send + 'static>(
    db: &Db,
    kind: &str,
) -> Result<Vec<T>, DbError> {
    let kind = kind.to_string();
    let rows: Vec<(String, String)> = db
        .call(move |c| {
            let mut st = c.prepare("SELECT id, body FROM core_state WHERE kind = ?1")?;
            let it = st.query_map(params![kind], |r| Ok((r.get(0)?, r.get(1)?)))?;
            it.collect()
        })
        .await?;
    Ok(rows
        .into_iter()
        .filter_map(|(id, body)| match serde_json::from_str(&body) {
            Ok(v) => Some(v),
            Err(e) => {
                tracing::error!(%id, %e, "пропущена повреждённая запись учёта ядра");
                None
            }
        })
        .collect())
}

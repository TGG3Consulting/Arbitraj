//! Файл: storage/venue_state.rs
//! Что делает: факты, которые адаптер площадки нашёл сам и которые не нужно искать заново после перезапуска
//! (таблица venue_state: площадка, вид, id → значение текстом). Пример: блок развёртывания PoolManager V4,
//! найденный бинарным поиском по eth_getCode (venues/bsc_dex/pools/deploy_block.rs).
//! Кто вызывает: venues/bsc_dex/pools/deploy_block.rs.
//! От чего зависит: storage/db.rs.
//! Настройки: нет.

use chrono::Utc;
use rusqlite::{OptionalExtension, params};

use super::db::{Db, DbError};

/// Сохранённое значение. None — не сохранялось. Ошибки: база.
pub async fn load(db: &Db, venue: &str, kind: &str, id: &str) -> Result<Option<String>, DbError> {
    let (v, k, i) = (venue.to_string(), kind.to_string(), id.to_string());
    db.call(move |c| {
        c.query_row(
            "SELECT body FROM venue_state WHERE venue = ?1 AND kind = ?2 AND id = ?3",
            params![v, k, i],
            |r| r.get(0),
        )
        .optional()
    })
    .await
}

/// Сохраняет значение (заменяет прежнее). Ошибки: база.
pub async fn save(db: &Db, venue: &str, kind: &str, id: &str, body: String) -> Result<(), DbError> {
    let (v, k, i) = (venue.to_string(), kind.to_string(), id.to_string());
    let at = Utc::now().to_rfc3339();
    db.call(move |c| {
        c.execute(
            "INSERT INTO venue_state (venue, kind, id, body, updated_at) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(venue, kind, id) DO UPDATE SET body = excluded.body, updated_at = excluded.updated_at",
            params![v, k, i, body, at],
        )
        .map(|_| ())
    })
    .await
}

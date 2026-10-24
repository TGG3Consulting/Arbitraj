//! Файл: storage/wallet_queue.rs
//! Что делает: состояние очереди транзакций кошелька DEX в SQLite (таблица wallet_queue: площадка → JSON
//! WalletQueueState) — чтобы «остановлена до ручной проверки» (и «ждёт итога» во время перезапуска) пережила перезапуск.
//! Кто вызывает: venues/bsc_dex/wallet/queue_store.rs.
//! От чего зависит: storage/db.rs.
//! Настройки: нет.

use chrono::Utc;
use rusqlite::{OptionalExtension, params};

use super::db::{Db, DbError};

/// Сохранённое состояние площадки (JSON). None — не сохранялось. Ошибки: база.
pub async fn load(db: &Db, venue: &str) -> Result<Option<String>, DbError> {
    let v = venue.to_string();
    db.call(move |c| {
        c.query_row(
            "SELECT body FROM wallet_queue WHERE venue = ?1",
            params![v],
            |r| r.get(0),
        )
        .optional()
    })
    .await
}

/// Сохраняет состояние площадки (JSON). Ошибки: база.
pub async fn save(db: &Db, venue: &str, body: String) -> Result<(), DbError> {
    let v = venue.to_string();
    let at = Utc::now().to_rfc3339();
    db.call(move |c| {
        c.execute(
            "INSERT INTO wallet_queue (venue, body, updated_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(venue) DO UPDATE SET body = excluded.body, updated_at = excluded.updated_at",
            params![v, body, at],
        )
        .map(|_| ())
    })
    .await
}

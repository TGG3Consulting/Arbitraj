//! Файл: storage/notify_mirror.rs
//! Что делает: решение 251, ответ (б) Тиграна 10.11 — сообщения дубля «Ручного управления» в Telegram в SQLite
//! (таблица notify_mirror: ключ записи → JSON строки notify/mirror_store.rs::Row — чат, id сообщений, запись, текст),
//! чтобы после перезапуска бот правил старые сообщения (editMessageText), а не слал новые. Строка живёт, пока запись
//! на экране «Ручное управление»; запись ушла (выполнена, закрыта) — строка удаляется.
//! Кто вызывает: notify/mirror_store.rs.
//! От чего зависит: storage/db.rs.
//! Настройки: нет.

use chrono::Utc;
use rusqlite::params;

use super::db::{Db, DbError};

/// Все строки: [(ключ записи, JSON)]. Ошибки: база.
pub async fn load_all(db: &Db) -> Result<Vec<(String, String)>, DbError> {
    db.call(|c| {
        let mut st = c.prepare("SELECT key, body FROM notify_mirror")?;
        let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        rows.collect()
    })
    .await
}

/// Одной транзакцией: записывает (заменяет) строки put и удаляет ключи del. Принимает: [(ключ, JSON)], [ключ].
/// Ошибки: база.
pub async fn apply(db: &Db, put: Vec<(String, String)>, del: Vec<String>) -> Result<(), DbError> {
    let at = Utc::now().to_rfc3339();
    db.call(move |c| {
        let tx = c.transaction()?;
        for (key, body) in put {
            tx.execute(
                "INSERT INTO notify_mirror (key, body, updated_at) VALUES (?1, ?2, ?3)
                 ON CONFLICT(key) DO UPDATE SET body = excluded.body, updated_at = excluded.updated_at",
                params![key, body, at],
            )?;
        }
        for key in del {
            tx.execute("DELETE FROM notify_mirror WHERE key = ?1", params![key])?;
        }
        tx.commit()
    })
    .await
}

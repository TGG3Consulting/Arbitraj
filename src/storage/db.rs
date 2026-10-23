//! Файл: storage/db.rs
//! Что делает: открывает базу SQLite, создаёт таблицы, даёт безопасный доступ из async-кода.
//! Запросы выполняются в отдельном потоке (spawn_blocking), чтобы не тормозить рыночные задачи.
//! Кто вызывает: main.rs (открывает), settings/store.rs, storage/journal.rs, storage/rules.rs.
//! От чего зависит: rusqlite, tokio.
//! Настройки: путь к папке данных — аргумент запуска --data-dir (до старта настроек его взять негде).

use std::path::Path;
use std::sync::{Arc, Mutex};

use rusqlite::Connection;

/// Схема базы. Таблицы создаются, если их нет.
const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS settings (
    name  TEXT NOT NULL,
    scope TEXT NOT NULL,
    value TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (name, scope)
);
CREATE TABLE IF NOT EXISTS settings_history (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL,
    scope TEXT NOT NULL,
    old_value TEXT,
    new_value TEXT,
    changed_by TEXT NOT NULL,
    changed_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS rules (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    body TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS journal (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    at TEXT NOT NULL,
    level TEXT NOT NULL,
    kind TEXT NOT NULL,
    venue TEXT,
    message TEXT NOT NULL,
    data TEXT
);
";

/// Ошибка базы.
#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("SQLite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("Поток базы завершился с ошибкой: {0}")]
    Join(String),
    #[error("Доступ к базе заблокирован после сбоя другого запроса")]
    Poisoned,
}

/// Общий доступ к базе. Дешёво клонируется.
#[derive(Clone)]
pub struct Db {
    conn: Arc<Mutex<Connection>>,
}

impl Db {
    /// Открывает (или создаёт) файл базы и таблицы.
    /// Принимает: путь к файлу. Ошибки: нет доступа к файлу, повреждённая база.
    pub fn open(path: &Path) -> Result<Self, DbError> {
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL;")?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// Выполняет функцию с соединением в отдельном потоке.
    /// Принимает: функцию над &mut Connection. Возвращает: её результат.
    /// Ошибки: ошибка SQLite из функции, сбой потока.
    pub async fn call<T, F>(&self, f: F) -> Result<T, DbError>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> Result<T, rusqlite::Error> + Send + 'static,
    {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            let mut guard = conn.lock().map_err(|_| DbError::Poisoned)?;
            f(&mut guard).map_err(DbError::from)
        })
        .await
        .map_err(|e| DbError::Join(e.to_string()))?
    }
}

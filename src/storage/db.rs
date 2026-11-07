//! Файл: storage/db.rs
//! Что делает: открывает базу SQLite, создаёт таблицы, даёт безопасный доступ из async-кода.
//! Запросы выполняются в отдельном потоке (spawn_blocking), чтобы не тормозить рыночные задачи.
//! Кто вызывает: main.rs (открывает), settings/store.rs, storage/journal.rs, storage/rules.rs.
//! От чего зависит: rusqlite, tokio.
//! Миграции: новые столбцы старых таблиц — ADDED_COLUMNS (добавляются при открытии, если их нет).
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
CREATE TABLE IF NOT EXISTS wallet_queue (
    venue TEXT PRIMARY KEY,
    body  TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS daily_result (
    day  TEXT PRIMARY KEY,
    body TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS core_state (
    kind TEXT NOT NULL,
    id   TEXT NOT NULL,
    body TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (kind, id)
);
CREATE TABLE IF NOT EXISTS episodes (
    id TEXT PRIMARY KEY,
    started_at TEXT NOT NULL,
    body TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS windows (
    id TEXT PRIMARY KEY,
    started_at TEXT NOT NULL,
    ticker TEXT NOT NULL,
    deal INTEGER NOT NULL,
    outcome TEXT,
    body TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS uncapped (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    kind TEXT NOT NULL,
    deal_id TEXT,
    ticker TEXT NOT NULL,
    started_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    body TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS uncapped_started ON uncapped (started_at);
CREATE INDEX IF NOT EXISTS uncapped_deal ON uncapped (deal_id);
CREATE TABLE IF NOT EXISTS venue_state (
    venue TEXT NOT NULL,
    kind  TEXT NOT NULL,
    id    TEXT NOT NULL,
    body  TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (venue, kind, id)
);
CREATE TABLE IF NOT EXISTS dex_wallets (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    secret TEXT NOT NULL,
    executor TEXT,
    queue_key TEXT NOT NULL,
    created_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS dex_wallet_map (
    row TEXT PRIMARY KEY,
    wallet TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS dex_wallet_meta (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS dex_wallet_history (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    what TEXT NOT NULL,
    target TEXT NOT NULL,
    old_value TEXT,
    new_value TEXT,
    changed_by TEXT NOT NULL,
    changed_at TEXT NOT NULL
);
";

/// Столбцы, добавленные к уже существующим таблицам: (таблица, столбец, тип). Старая база получает их при открытии
/// (ALTER TABLE … ADD COLUMN), новая — так же сразу после CREATE TABLE. Решение 155: журнал — счётчик повторов одной
/// причины (`repeats`) и время последнего повтора (`last_at`); у обычных записей оба NULL. Решение 211 (монета = адрес):
/// адрес контракта монеты (`address`, 0x… в нижнем регистре) у окон и записей «без потолка»; у старых строк NULL.
const ADDED_COLUMNS: &[(&str, &str, &str)] = &[
    ("journal", "repeats", "INTEGER"),
    ("journal", "last_at", "TEXT"),
    ("windows", "address", "TEXT"),
    ("uncapped", "address", "TEXT"),
];

/// Миграция: добавляет недостающие столбцы ADDED_COLUMNS. Принимает: соединение. Ошибки: SQLite.
fn migrate(conn: &Connection) -> Result<(), rusqlite::Error> {
    for (table, col, ty) in ADDED_COLUMNS {
        let mut st = conn.prepare(&format!("SELECT name FROM pragma_table_info('{table}')"))?;
        let have: Vec<String> = st.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?;
        if !have.iter().any(|c| c == col) {
            conn.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN {col} {ty};"))?;
        }
    }
    Ok(())
}

/// Ошибка базы.
#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("SQLite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("Поток базы завершился с ошибкой: {0}")]
    Join(String),
    #[error("Не удалось преобразовать данные: {0}")]
    Json(String),
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
        migrate(&conn)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// Что делает: решение 218 — открывает УЖЕ СУЩЕСТВУЮЩУЮ базу бота для внешнего инструмента (src/bin/
    /// uncapped_recompute.rs), без создания таблиц и миграций. exclusive = false — только чтение (бот может работать).
    /// exclusive = true — чтение и запись с исключительной блокировкой файла (PRAGMA locking_mode = EXCLUSIVE и сразу
    /// BEGIN EXCLUSIVE, ожидание 0): база в WAL, и пока бот запущен, его соединение держит разделяемую блокировку — тогда
    /// отказ «база занята»; взятая блокировка держится до закрытия инструмента — бот, запущенный в это время, базу не
    /// откроет (проверено: python sqlite3, два процесса, 07.11.2026).
    /// Принимает: путь, режим. Ошибки: файла нет, база занята (Sqlite Busy), повреждённая база.
    pub fn open_existing(path: &Path, exclusive: bool) -> Result<Self, DbError> {
        use rusqlite::OpenFlags;
        let flags = if exclusive {
            OpenFlags::SQLITE_OPEN_READ_WRITE
        } else {
            OpenFlags::SQLITE_OPEN_READ_ONLY
        } | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_URI;
        let conn = Connection::open_with_flags(path, flags)?;
        if exclusive {
            conn.busy_timeout(std::time::Duration::ZERO)?;
            conn.execute_batch("PRAGMA locking_mode=EXCLUSIVE; BEGIN EXCLUSIVE; COMMIT;")?;
        }
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

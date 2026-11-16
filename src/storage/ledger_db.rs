//! Файл: storage/ledger_db.rs
//! Что делает: решение 353 (Тигран 16.11: «Сделка имеет своё отдельное соединение и ни с кем не конкурирует») — учёт
//! ядра в ОТДЕЛЬНОМ файле SQLite рядом с основной базой (LEDGER_FILE): таблицы core_state (сделки и их шаги, переводы,
//! газ, ручное управление), daily_result (итог дня), wallet_queue (очередь транзакций кошелька DEX) — всё, что нужно
//! для восстановления после сбоя. По документации SQLite (https://www.sqlite.org/wal.html, раздел «Concurrency»)
//! писатель в режиме WAL один на ФАЙЛ: второе соединение к тому же файлу ждало бы конца чужой транзакции (журнала,
//! окон, отчётов). Поэтому только свой файл гарантирует, что запись учёта не ждёт ни одной чужой записи: у этого файла
//! нет других писателей.
//!
//! Устройство: писатель — своё соединение в своём потоке ОС (не пул блокирующих задач tokio); запросы по порядку
//! постановки; вызывающий ждёт только коммита своей короткой транзакции (сделка — одна строка). Чтение — второе
//! соединение (WAL: читатели и писатель не ждут друг друга, читатель видит последнее подтверждённое состояние).
//! Долговечность — PRAGMA synchronous писателя (настройка ledger.synchronous; не задана — значение SQLite по умолчанию).
//! Прямой режим (direct) — для внешних инструментов (src/bin, Db::open_existing): одно соединение, без потока.
//! Перенос из основной базы — storage/ledger_move.rs (при каждом открытии, только недостающее или более новое).
//! Кто вызывает: storage/db.rs (open, open_existing, ledger), core/persist.rs, core/daily_*.rs, storage/wallet_queue.rs,
//! core/usdt_sent_backfill.rs, storage/write_modes.rs (set_synchronous).
//! От чего зависит: rusqlite, std::sync::mpsc, tokio (oneshot, spawn_blocking), storage/ledger_move.rs.
//! Настройки: ledger.synchronous (передаёт storage/write_modes.rs).

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex};

use rusqlite::Connection;
use tokio::sync::oneshot;

use super::db::DbError;

/// Имя файла учёта ядра рядом с основной базой (main.rs DB_FILE).
pub const LEDGER_FILE: &str = "ledger.sqlite";

/// Схема файла учёта: те же таблицы, что в основной базе (storage/db.rs SCHEMA), — код читает и пишет их как раньше.
pub(super) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS daily_result (
    day  TEXT PRIMARY KEY,
    body TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS wallet_queue (
    venue TEXT PRIMARY KEY,
    body  TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS core_state (
    kind TEXT NOT NULL,
    id   TEXT NOT NULL,
    body TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (kind, id)
);
";

/// Задание писателю: функция над его соединением (ответ — внутри, через oneshot).
type Job = Box<dyn FnOnce(&mut Connection) + Send>;

/// Учёт ядра. Дешёво клонируется; последний клон удалён — поток писателя завершается.
#[derive(Clone)]
pub struct LedgerDb {
    /// Поток писателя (бот); None — прямой режим: запись тем же соединением, что чтение.
    writer: Option<Sender<Job>>,
    /// Соединение чтения (в прямом режиме — единственное).
    read: Arc<Mutex<Connection>>,
    /// synchronous писателя при открытии (значение SQLite по умолчанию) — для снятия настройки ledger.synchronous.
    sync0: Option<i64>,
    /// Сколько строк перенесено из основной базы при открытии (ledger_move.rs).
    moved: usize,
}

impl LedgerDb {
    /// Что делает: открывает (создаёт) файл учёта: WAL, таблицы, перенос недостающего из основной базы, поток писателя
    /// и соединение чтения. Принимает: путь файла учёта, соединение основной базы (только чтение для переноса).
    /// Ошибки: файл, SQLite, поток не запустился.
    pub fn open(path: &Path, main: &Connection) -> Result<Self, DbError> {
        let mut w = Connection::open(path)?;
        w.execute_batch("PRAGMA journal_mode=WAL;")?;
        w.execute_batch(SCHEMA)?;
        let sync0: i64 = w.pragma_query_value(None, "synchronous", |r| r.get(0))?;
        let moved = super::ledger_move::from_main(main, &mut w)?;
        let read = Connection::open(path)?;
        read.execute_batch("PRAGMA query_only=ON;")?;
        let (tx, rx) = channel::<Job>();
        std::thread::Builder::new()
            .name("ledger-db".into())
            .spawn(move || {
                let mut w = w;
                // Задания по порядку постановки; выход — когда все клоны удалены.
                while let Ok(job) = rx.recv() {
                    job(&mut w);
                }
            })
            .map_err(|e| DbError::Join(e.to_string()))?;
        Ok(Self {
            writer: Some(tx),
            read: Arc::new(Mutex::new(read)),
            sync0: Some(sync0),
            moved,
        })
    }

    /// Что делает: прямой режим — одно соединение для чтения и записи, без потока (внешние инструменты src/bin: файл
    /// учёта, открытый как основная база, или сама основная база, если файла учёта ещё нет). Принимает: соединение.
    /// Ошибок нет.
    pub fn direct(conn: Arc<Mutex<Connection>>) -> Self {
        Self {
            writer: None,
            read: conn,
            sync0: None,
            moved: 0,
        }
    }

    /// Сколько строк перенесено из основной базы при открытии. Ошибок нет.
    pub fn moved(&self) -> usize {
        self.moved
    }

    /// Что делает: запись учёта — функция над соединением писателя; ждётся только её коммит (задания писателя идут по
    /// порядку постановки, у файла других писателей нет). Принимает: функцию. Возвращает: её результат.
    /// Ошибки: SQLite, поток писателя завершился.
    pub async fn write<T, F>(&self, f: F) -> Result<T, DbError>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> Result<T, rusqlite::Error> + Send + 'static,
    {
        let Some(tx) = &self.writer else {
            return self.read(f).await;
        };
        let (reply, rx) = oneshot::channel();
        let job: Job = Box::new(move |c| {
            // Ответ никто не ждёт (вызывающий отменён) — не ошибка: запись уже сделана.
            let _ = reply.send(f(c).map_err(DbError::from));
        });
        tx.send(job).map_err(|_| gone())?;
        rx.await.map_err(|_| gone())?
    }

    /// Что делает: чтение учёта — функция над соединением чтения в отдельном потоке (spawn_blocking); писателя не
    /// ждёт (WAL). Принимает: функцию. Возвращает: её результат. Ошибки: SQLite, сбой потока, замок отравлен.
    pub async fn read<T, F>(&self, f: F) -> Result<T, DbError>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> Result<T, rusqlite::Error> + Send + 'static,
    {
        let conn = self.read.clone();
        tokio::task::spawn_blocking(move || {
            let mut guard = conn.lock().map_err(|_| DbError::Poisoned)?;
            f(&mut guard).map_err(DbError::from)
        })
        .await
        .map_err(|e| DbError::Join(e.to_string()))?
    }

    /// Что делает: PRAGMA synchronous писателя (https://www.sqlite.org/pragma.html#pragma_synchronous) — заданием по
    /// порядку с записями. None — значение при открытии (SQLite по умолчанию). Прямой режим — ничего.
    /// Принимает: значение (NORMAL / FULL / EXTRA). Ошибок нет: ошибка SQLite — в лог процесса.
    pub fn set_synchronous(&self, v: Option<String>) {
        let (Some(tx), Some(sync0)) = (&self.writer, self.sync0) else {
            return;
        };
        let job: Job = Box::new(move |c| {
            let r = match &v {
                Some(s) => c.pragma_update(None, "synchronous", s),
                None => c.pragma_update(None, "synchronous", sync0),
            };
            if let Err(e) = r {
                tracing::error!(%e, ?v, "ledger.sqlite: PRAGMA synchronous не применена");
            }
        });
        if tx.send(job).is_err() {
            tracing::error!("ledger.sqlite: поток писателя завершился — synchronous не применена");
        }
    }
}

/// Ошибка «поток писателя учёта завершился». Ошибок нет.
fn gone() -> DbError {
    DbError::Join("поток записи учёта ядра (ledger.sqlite) завершился".into())
}

/// Путь файла учёта рядом с основной базой. Принимает: путь основной базы. Ошибок нет.
pub fn path_near(main: &Path) -> PathBuf {
    main.with_file_name(LEDGER_FILE)
}

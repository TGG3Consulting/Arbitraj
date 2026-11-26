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
//! Перенос из основной базы — storage/ledger_move.rs (один раз, с отметкой в таблице meta этого файла).
//! Решение 397: закрытие детерминировано, как у очереди журнала (journal_queue.rs shutdown) — последний клон удалён:
//! канал закрыт, поток дописывает всё, что уже в канале, закрывает своё соединение и завершается; удаляющий ждёт его
//! (Writer::drop). Иначе соединение писателя могло жить после закрытия Db, и следующее исключительное открытие
//! (Db::open_existing(true), ожидание 0) получало «база занята» (CI, tests/ledger_db_353.rs).
//! Кто вызывает: storage/db.rs (open, open_existing, ledger), core/persist.rs, core/daily_*.rs, storage/wallet_queue.rs,
//! core/usdt_sent_backfill.rs, storage/write_modes.rs (set_synchronous).
//! От чего зависит: rusqlite, std::sync::mpsc, std::thread, tokio (oneshot, spawn_blocking), storage/ledger_move.rs.
//! Настройки: ledger.synchronous (передаёт storage/write_modes.rs).

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

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
CREATE TABLE IF NOT EXISTS meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS gate_withdraw_denial (
 account TEXT NOT NULL, asset TEXT NOT NULL, network TEXT NOT NULL, address TEXT NOT NULL, rejected_at INTEGER NOT NULL,
 PRIMARY KEY(account,asset,network,address)
);
CREATE TABLE IF NOT EXISTS cex_funds (
    action TEXT PRIMARY KEY, bucket TEXT NOT NULL, client TEXT NOT NULL, deal TEXT NOT NULL,
    funding INTEGER NOT NULL, cap TEXT NOT NULL, order_id TEXT, debit TEXT,
    UNIQUE(bucket,client), UNIQUE(bucket,order_id)
);
CREATE INDEX IF NOT EXISTS cex_funds_bucket ON cex_funds(bucket);
CREATE TABLE IF NOT EXISTS cex_funds_epoch (bucket TEXT PRIMARY KEY, revision INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS deposit_log_claim (
    chain TEXT NOT NULL, token TEXT NOT NULL, wallet TEXT NOT NULL, tx TEXT NOT NULL,
    log_index INTEGER NOT NULL, sender TEXT NOT NULL, withdrawal_id TEXT NOT NULL,
    deal TEXT NOT NULL, raw_amount TEXT NOT NULL,
    PRIMARY KEY (chain, token, wallet, tx, log_index),
    UNIQUE (sender, withdrawal_id, chain, wallet)
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

/// Решение 397: поток писателя — отправитель заданий и ручка потока; один на все клоны LedgerDb.
struct Writer {
    /// Отправитель; None только внутри drop (канал закрыт).
    tx: Option<Sender<Job>>,
    /// Поток писателя; None — уже дождались (или внутри drop).
    thread: Option<JoinHandle<()>>,
}

impl Drop for Writer {
    /// Что делает: последний клон LedgerDb удалён (база закрывается) — закрыть канал, дождаться потока: он выполняет
    /// задания, уже стоящие в канале (std::sync::mpsc отдаёт их и после закрытия), и закрывает соединение ledger.sqlite.
    /// Удаление из самого писателя (задание держало последний клон) — без ожидания. Ошибок нет: паника потока
    /// (вне заданий — их ловит catch_unwind) — без действий.
    /// Где вызывается на живом боте: Db держат задачи и состояние веб-сервера до конца процесса; остановка по сигналу
    /// выходит через std::process::exit (core/shutdown.rs) — деструкторы не идут, ожидания нет. Drop бывает при
    /// выходе из main с ошибкой (рантайм гасит задачи) и во внешних инструментах/тестах; ждётся только хвост уже
    /// поставленных коротких транзакций учёта (сделка — одна строка), рыночные задачи к этому моменту не работают.
    fn drop(&mut self) {
        drop(self.tx.take());
        if let Some(h) = self.thread.take()
            && h.thread().id() != std::thread::current().id()
        {
            let _ = h.join();
        }
    }
}

/// Учёт ядра. Дешёво клонируется; последний клон удалён — поток писателя дописывает канал и завершается, удаляющий его
/// ждёт (решение 397, Writer::drop).
#[derive(Clone)]
pub struct LedgerDb {
    /// Поток писателя (бот); None — прямой режим: запись тем же соединением, что чтение.
    writer: Option<Arc<Writer>>,
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
        let thread = std::thread::Builder::new()
            .name("ledger-db".into())
            .spawn(move || {
                let mut w = w;
                // Задания по порядку постановки; выход — канал закрыт (Writer::drop) и пуст; соединение закрывается
                // здесь, до конца потока. Ревью t158 Н4: паника задания не останавливает поток — вызывающий получает
                // ошибку (ответ не пришёл), следующие записи идут дальше.
                while let Ok(job) = rx.recv() {
                    let run = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| job(&mut w)));
                    if run.is_err() {
                        tracing::error!("ledger.sqlite: сбой (паника) задания записи учёта — задание не выполнено");
                    }
                }
            })
            .map_err(|e| DbError::Join(e.to_string()))?;
        Ok(Self {
            writer: Some(Arc::new(Writer {
                tx: Some(tx),
                thread: Some(thread),
            })),
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
    /// Ошибки: SQLite, поток писателя завершился (канал закрыт).
    pub async fn write<T, F>(&self, f: F) -> Result<T, DbError>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> Result<T, rusqlite::Error> + Send + 'static,
    {
        if self.writer.is_none() {
            return self.read(f).await;
        }
        let (reply, rx) = oneshot::channel();
        let job: Job = Box::new(move |c| {
            // Ответ никто не ждёт (вызывающий отменён) — не ошибка: запись уже сделана.
            let _ = reply.send(f(c).map_err(DbError::from));
        });
        // Отправка — синхронной функцией: ссылка на канал не держится через await (сторожа стека будущих сделки).
        self.send(job)?;
        rx.await.map_err(|_| gone())?
    }

    /// Что делает: ставит задание в канал писателя. Принимает: задание. Ошибки: прямой режим или канал закрыт
    /// (поток писателя завершился) — gone.
    fn send(&self, job: Job) -> Result<(), DbError> {
        let tx = self.writer.as_ref().and_then(|w| w.tx.as_ref());
        tx.ok_or_else(gone)?.send(job).map_err(|_| gone())
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
        let (true, Some(sync0)) = (self.writer.is_some(), self.sync0) else {
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
        if self.send(job).is_err() {
            tracing::error!("ledger.sqlite: поток писателя завершился — synchronous не применена");
        }
    }
}

/// Ошибка «запись учёта не выполнена» (поток писателя завершился или задание упало). Ошибок нет.
fn gone() -> DbError {
    DbError::Join("запись учёта ядра (ledger.sqlite) не выполнена: сбой потока записи".into())
}

/// Путь файла учёта рядом с основной базой. Принимает: путь основной базы. Ошибок нет.
pub fn path_near(main: &Path) -> PathBuf {
    main.with_file_name(LEDGER_FILE)
}

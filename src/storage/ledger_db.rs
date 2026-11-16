//! Файл: storage/ledger_db.rs
//! Что делает: решение 342 (Тигран 16.11: «Сделка имеет своё отдельное соединение и ни с кем не конкурирует») — учёт
//! сделок в ОТДЕЛЬНОМ файле SQLite рядом с основной базой (LEDGER_FILE, таблица core_state той же схемы) со своим
//! соединением и своим потоком ОС. В SQLite писатель один на ФАЙЛ, поэтому только отдельный файл гарантирует, что ни
//! одна запись журнала, окна, эпизода, настроек или отчёта не встанет перед записью сделки (у этого файла нет других
//! писателей). Запросы идут по порядку постановки; сохранение отвечает после коммита — сделка ждёт только свою
//! запись (долговечность — как PRAGMA synchronous этого соединения, настройка ledger.synchronous).
//! Включается настройкой ledger.own_db (storage/write_modes.rs → Db::set_ledger). Соединение одно на весь запуск бота:
//! выключение только перестаёт направлять сюда записи (соединение и поток живут дальше), повторное включение берёт то
//! же соединение — у файла никогда нет двух писателей. Смена ledger.synchronous — PRAGMA тем же потоком по порядку;
//! снятие настройки (не задана) PRAGMA не меняет — прежнее значение действует до перезапуска. Выключено — файл не
//! пишется, но читается при загрузке (core/persist.rs load: сделки из обоих файлов, по каждой — запись с последним updated_at),
//! поэтому включение и выключение на ходу не теряют ни одной сделки и не требуют переноса старой базы.
//! Кто вызывает: storage/db.rs (set_ledger, ledger, ledger_rows), core/persist.rs (через Db).
//! От чего зависит: rusqlite, std::sync::mpsc, tokio::sync::oneshot.
//! Настройки: ledger.own_db, ledger.synchronous (читает storage/write_modes.rs).

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Sender, channel};

use chrono::Utc;
use rusqlite::{Connection, OpenFlags, params};
use tokio::sync::oneshot;

use super::db::DbError;

/// Имя файла учёта сделок рядом с основной базой (как имя основной базы — main.rs DB_FILE).
pub const LEDGER_FILE: &str = "ledger.sqlite";

/// Схема файла учёта: та же таблица core_state, что в основной базе (storage/db.rs SCHEMA).
const SCHEMA: &str = "CREATE TABLE IF NOT EXISTS core_state (
    kind TEXT NOT NULL,
    id   TEXT NOT NULL,
    body TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (kind, id)
);";

/// Строка учёта: id, тело JSON, updated_at (RFC 3339).
pub type Row = (String, String, String);

/// Запрос потоку учёта.
enum Req {
    Save {
        kind: String,
        id: String,
        body: String,
        reply: oneshot::Sender<Result<(), DbError>>,
    },
    Load {
        kind: String,
        reply: oneshot::Sender<Result<Vec<Row>, DbError>>,
    },
    /// Новое значение PRAGMA synchronous — по порядку с записями.
    Sync(String),
}

/// Соединение учёта сделок (поток ОС). Дешёво клонируется; последний клон удалён — поток дописывает и завершается.
#[derive(Clone)]
pub struct LedgerDb {
    tx: Sender<Req>,
}

impl LedgerDb {
    /// Открывает (создаёт) файл учёта: WAL, synchronous (если задан), таблица; запускает поток. Принимает: путь,
    /// значение synchronous (OFF / NORMAL / FULL / EXTRA — https://www.sqlite.org/pragma.html#pragma_synchronous).
    /// Ошибки: файл, SQLite, поток не запустился.
    pub fn open(path: &Path, synchronous: Option<String>) -> Result<Self, DbError> {
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL;")?;
        if let Some(s) = &synchronous {
            conn.pragma_update(None, "synchronous", s)?;
        }
        conn.execute_batch(SCHEMA)?;
        let (tx, rx) = channel::<Req>();
        std::thread::Builder::new()
            .name("ledger-db".into())
            .spawn(move || {
                // Запросы по порядку постановки; выход — когда все клоны удалены.
                while let Ok(r) = rx.recv() {
                    serve(&conn, r);
                }
            })
            .map_err(|e| DbError::Join(e.to_string()))?;
        Ok(Self { tx })
    }

    /// Новое значение PRAGMA synchronous — выполняется потоком учёта после уже поставленных записей. Принимает:
    /// значение. Ошибок нет (ошибка SQLite — в лог процесса; соединение остаётся со старым значением).
    pub fn set_synchronous(&self, v: &str) {
        let _ = self.tx.send(Req::Sync(v.to_string()));
    }

    /// Сохраняет запись (новую или поверх) и ждёт коммита — только своей записи. Принимает: вид, id, тело JSON.
    /// Ошибки: SQLite, поток учёта завершился.
    pub async fn save(&self, kind: &str, id: &str, body: String) -> Result<(), DbError> {
        let (reply, rx) = oneshot::channel();
        let req = Req::Save {
            kind: kind.to_string(),
            id: id.to_string(),
            body,
            reply,
        };
        self.tx.send(req).map_err(|_| gone())?;
        rx.await.map_err(|_| gone())?
    }

    /// Все строки вида (через поток учёта — после всех поставленных записей). Ошибки: SQLite, поток завершился.
    pub async fn load(&self, kind: &str) -> Result<Vec<Row>, DbError> {
        let (reply, rx) = oneshot::channel();
        let req = Req::Load {
            kind: kind.to_string(),
            reply,
        };
        self.tx.send(req).map_err(|_| gone())?;
        rx.await.map_err(|_| gone())?
    }
}

/// Ошибка «поток учёта завершился». Ошибок нет.
fn gone() -> DbError {
    DbError::Join("поток учёта сделок (ledger.sqlite) завершился".into())
}

/// Выполняет один запрос в потоке учёта. Принимает: соединение, запрос. Ошибок нет (ответ — в канал запроса).
fn serve(c: &Connection, r: Req) {
    match r {
        Req::Save {
            kind,
            id,
            body,
            reply,
        } => {
            let res = c
                .execute(
                    "INSERT INTO core_state (kind, id, body, updated_at) VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT(kind, id) DO UPDATE SET body = excluded.body, updated_at = excluded.updated_at",
                    params![kind, id, body, Utc::now().to_rfc3339()],
                )
                .map(|_| ())
                .map_err(DbError::from);
            let _ = reply.send(res);
        }
        Req::Load { kind, reply } => {
            let _ = reply.send(rows(c, &kind).map_err(DbError::from));
        }
        Req::Sync(v) => {
            if let Err(e) = c.pragma_update(None, "synchronous", &v) {
                tracing::error!(%e, v, "ledger.sqlite: PRAGMA synchronous не применена");
            }
        }
    }
}

/// Строки вида из соединения. Принимает: соединение, вид. Ошибки: SQLite.
pub fn rows(c: &Connection, kind: &str) -> rusqlite::Result<Vec<Row>> {
    let mut st = c.prepare("SELECT id, body, updated_at FROM core_state WHERE kind = ?1")?;
    let it = st.query_map(params![kind], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
    it.collect()
}

/// Строки вида из файла учёта, когда соединения учёта нет (ledger.own_db выключена, или инструмент): только чтение;
/// файла нет — пусто. Принимает: путь, вид. Ошибки: SQLite (файл есть, но не читается).
pub fn read_file(path: &Path, kind: &str) -> Result<Vec<Row>, DbError> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let c = Connection::open_with_flags(path, flags)?;
    Ok(rows(&c, kind)?)
}

/// Путь файла учёта рядом с основной базой. Принимает: путь основной базы. Ошибок нет.
pub fn path_near(main: &Path) -> PathBuf {
    main.with_file_name(LEDGER_FILE)
}

//! Файл: storage/db_ledger.rs
//! Что делает: решение 342 — методы Db для учёта сделок в своём файле (storage/ledger_db.rs): включить / выключить
//! своё соединение (ledger.own_db, ledger.synchronous), взять его, прочитать строки файла учёта. Вынесено из db.rs.
//! Кто вызывает: storage/write_modes.rs (set_ledger), core/persist.rs (ledger, ledger_rows).
//! От чего зависит: storage/db.rs (поля ledger_path, ledger), storage/ledger_db.rs, tokio (spawn_blocking).
//! Настройки: ledger.own_db, ledger.synchronous (значения передаёт write_modes.rs).

use super::db::{Db, DbError};
use super::ledger_db::{self, LedgerDb, Row};

/// Своё соединение учёта: открыто ли (живёт до конца запуска), направлять ли записи, применённый synchronous.
#[derive(Default)]
pub struct Own {
    conn: Option<LedgerDb>,
    on: bool,
    sync: Option<String>,
}

impl Db {
    /// Что делает: решение 342 — включает (on = да) или выключает запись сделок своим соединением (отдельный файл,
    /// storage/ledger_db.rs). Соединение открывается при первом включении и живёт до конца запуска (выключение только
    /// перестаёт направлять записи — двух писателей у файла не бывает); synchronous задан и сменился — PRAGMA тем же
    /// потоком. Принимает: включено ли, PRAGMA synchronous (None — не менять). Ошибки: файл не открылся (учёт остаётся
    /// в основной базе).
    pub fn set_ledger(&self, on: bool, synchronous: Option<String>) -> Result<(), DbError> {
        let mut g = self.ledger.lock().map_err(|_| DbError::Poisoned)?;
        if on && g.conn.is_none() {
            g.conn = Some(LedgerDb::open(&self.ledger_path, synchronous.clone())?);
            g.sync = synchronous.clone();
        }
        if let (Some(c), Some(v)) = (&g.conn, &synchronous)
            && g.sync.as_ref() != Some(v)
        {
            c.set_synchronous(v);
            g.sync = synchronous.clone();
        }
        g.on = on && g.conn.is_some();
        Ok(())
    }

    /// Соединение учёта сделок, если включено (решение 342). Ошибок нет.
    pub fn ledger(&self) -> Option<LedgerDb> {
        let g = self.ledger.lock().ok()?;
        if g.on { g.conn.clone() } else { None }
    }

    /// Строки вида из файла учёта сделок (решение 342): своим соединением, если включено, иначе только чтением файла
    /// (нет файла — пусто). Принимает: вид. Ошибки: SQLite, поток учёта.
    pub async fn ledger_rows(&self, kind: &str) -> Result<Vec<Row>, DbError> {
        // Соединение есть (включено сейчас или раньше в этом запуске) — читаем им: по порядку с его записями.
        let own = self.ledger.lock().ok().and_then(|g| g.conn.clone());
        if let Some(l) = own {
            return l.load(kind).await;
        }
        let (path, kind) = (self.ledger_path.clone(), kind.to_string());
        tokio::task::spawn_blocking(move || ledger_db::read_file(&path, &kind))
            .await
            .map_err(|e| DbError::Join(e.to_string()))?
    }
}

//! Файл: storage/write_modes.rs
//! Что делает: режимы записи хранилища из настроек, на лету. Решение 341 — journal.queue_max: очередь записи журнала
//! (Journal::set_queue_max; при первом включении — обработчик остановки journal_stop.rs). Решение 342 — ledger.own_db и
//! ledger.synchronous: учёт сделок своим соединением в своём файле (Db::set_ledger, storage/ledger_db.rs). Не заданы —
//! как раньше. Смена настройки применяется сразу (подписка на изменения).
//! Кто вызывает: main.rs (apply при запуске до адаптеров, run — задача на всё время работы).
//! От чего зависит: storage/journal.rs, storage/db_ledger.rs, storage/journal_stop.rs, settings (Store, Watch).
//! Настройки: journal.queue_max, ledger.own_db, ledger.synchronous (все необязательные).

use crate::settings::live::Watch;
use crate::settings::read::At;
use crate::settings::store::Store;

use super::db::Db;
use super::journal::{Journal, Level};

/// Предел очереди записи журнала (решение 341).
pub const JOURNAL_MAX: &str = "journal.queue_max";
/// Учёт сделок своим соединением в своём файле (решение 342).
pub const LEDGER_OWN: &str = "ledger.own_db";
/// PRAGMA synchronous соединения учёта сделок (решение 342).
pub const LEDGER_SYNC: &str = "ledger.synchronous";

/// Применяет текущие значения настроек. Принимает: журнал, базу, настройки. Ошибок нет: файл учёта не открылся —
/// запись журнала (Error), учёт остаётся в основной базе.
pub fn apply(journal: &Journal, db: &Db, settings: &Store) {
    let max = settings.integer(JOURNAL_MAX, At::default()).ok();
    journal.set_queue_max(max);
    if max.is_some_and(|m| m > 0) {
        super::journal_stop::install(journal.clone());
    }
    let own = settings.flag(LEDGER_OWN, At::default()).unwrap_or(false);
    let sync = settings
        .text(LEDGER_SYNC, At::default())
        .ok()
        .filter(|s| !s.is_empty());
    if let Err(e) = db.set_ledger(own, sync) {
        let msg = format!("Учёт сделок в своём файле (ledger.own_db) не включён: {e} — учёт в основной базе");
        tracing::error!("{msg}");
        let j = journal.clone();
        tokio::spawn(async move {
            let _ = j.write(Level::Error, "storage.ledger", None, msg, None).await;
        });
    }
}

/// Работает всё время: изменения настроек режимов записи — применить сразу. Принимает: журнал, базу, настройки,
/// подписку на изменения (journal.queue_max, ledger.*). Ошибок нет.
pub async fn run(journal: Journal, db: Db, settings: Store, mut watch: Watch) {
    while watch.next().await.is_some() {
        apply(&journal, &db, &settings);
    }
}

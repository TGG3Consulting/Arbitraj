//! Файл: storage/write_modes.rs
//! Что делает: пределы и режимы записи хранилища из настроек, на лету. Решение 341 (очередь журнала работает всегда):
//! journal.queue_max — предел длины очереди (Journal::set_queue_max; не задан — без предела), journal.batch_max — предел
//! записей в одной транзакции пачки (Journal::set_batch_max; не задан — всё накопленное). Решение 353 —
//! ledger.synchronous: PRAGMA synchronous писателя файла учёта ядра (storage/ledger_db.rs; не задана — значение SQLite
//! при открытии). Смена настройки применяется сразу (подписка на изменения).
//! Кто вызывает: main.rs (apply при запуске до адаптеров, run — задача на всё время работы).
//! От чего зависит: storage/journal.rs, storage/db.rs (ledger), storage/ledger_db.rs,
//! settings (Store, Watch).
//! Настройки: journal.queue_max, journal.batch_max, ledger.synchronous (все необязательные).

use crate::settings::live::Watch;
use crate::settings::read::At;
use crate::settings::store::Store;

use super::db::Db;
use super::journal::Journal;

/// Предел длины очереди записи журнала (решение 341).
pub const JOURNAL_MAX: &str = "journal.queue_max";
/// Предел записей в одной транзакции пачки журнала (решение 341).
pub const JOURNAL_BATCH: &str = "journal.batch_max";
/// PRAGMA synchronous писателя файла учёта ядра (решение 353).
pub const LEDGER_SYNC: &str = "ledger.synchronous";

/// Имена настроек, на смену которых подписан run (main.rs). Ошибок нет.
pub fn names() -> Vec<String> {
    [JOURNAL_MAX, JOURNAL_BATCH, LEDGER_SYNC]
        .iter()
        .map(|s| s.to_string())
        .collect()
}

/// Применяет текущие значения настроек. Принимает: журнал, базу, настройки. Ошибок нет.
pub fn apply(journal: &Journal, db: &Db, settings: &Store) {
    journal.set_queue_max(settings.integer(JOURNAL_MAX, At::default()).ok());
    journal.set_batch_max(settings.integer(JOURNAL_BATCH, At::default()).ok());
    let sync = settings
        .text(LEDGER_SYNC, At::default())
        .ok()
        .filter(|s| !s.is_empty());
    db.ledger().set_synchronous(sync);
}

/// Работает всё время: изменения настроек режимов записи — применить сразу. Принимает: журнал, базу, настройки,
/// подписку на изменения (names). Ошибок нет.
pub async fn run(journal: Journal, db: Db, settings: Store, mut watch: Watch) {
    while watch.next().await.is_some() {
        apply(&journal, &db, &settings);
    }
}

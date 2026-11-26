//! Файл: storage/mod.rs
//! Что делает: хранение — база SQLite, журнал, правила шлюза.
//! Запись сырых сообщений площадок (zstd), файлы и список эпизодов арбитражной ситуации (решение 74), список окон (решение 125;
//! файлы окон — тем же episode_files.rs, своя папка); сообщения дубля «Ручного управления» в Telegram (решение 251 (б),
//! notify_mirror.rs).
//! Кто вызывает: main.rs, settings, api, notify.
//! От чего зависит: rusqlite.
//! Настройки: нет.

pub mod cex_funds;
pub mod db;
pub mod episode_files;
pub mod episodes_db;
pub mod journal;
mod journal_counted;
pub mod journal_op;
pub mod journal_queue;
mod journal_writer;
pub mod ledger_db;
mod ledger_move;
pub mod notify_mirror;
pub mod raw_log;
pub mod rules;
pub(crate) mod signals_archive;
pub mod uncapped_db;
pub mod venue_state;
pub mod wallet_queue;
pub mod windows_db;
pub mod write_modes;

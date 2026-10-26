//! Файл: storage/mod.rs
//! Что делает: хранение — база SQLite, журнал, правила шлюза.
//! Запись сырых сообщений площадок (zstd), файлы и список эпизодов арбитражной ситуации (решение 74).
//! Кто вызывает: main.rs, settings, api, notify.
//! От чего зависит: rusqlite.
//! Настройки: нет.

pub mod db;
pub mod episode_files;
pub mod episodes_db;
pub mod journal;
pub mod raw_log;
pub mod rules;
pub mod venue_state;
pub mod wallet_queue;

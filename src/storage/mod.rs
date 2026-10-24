//! Файл: storage/mod.rs
//! Что делает: хранение — база SQLite, журнал, правила шлюза.
//! Запись сырых сообщений площадок (zstd). Позже здесь же: сделки и переводы.
//! Кто вызывает: main.rs, settings, api, notify.
//! От чего зависит: rusqlite.
//! Настройки: нет.

pub mod db;
pub mod journal;
pub mod raw_log;
pub mod rules;
pub mod wallet_queue;

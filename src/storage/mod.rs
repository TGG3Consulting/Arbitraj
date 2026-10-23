//! Файл: storage/mod.rs
//! Что делает: хранение — база SQLite, журнал, правила шлюза.
//! Позже здесь же: сделки и переводы, запись сырых сообщений площадок (zstd).
//! Кто вызывает: main.rs, settings, api, notify.
//! От чего зависит: rusqlite.
//! Настройки: нет.

pub mod db;
pub mod journal;
pub mod rules;

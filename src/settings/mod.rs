//! Файл: settings/mod.rs
//! Что делает: хранилище настроек — каталог, проверка, SQLite с историей, изменения на лету.
//! Кто вызывает: main.rs, api, notify, позже ядро и адаптеры.
//! От чего зависит: storage/db.rs.
//! Настройки: все настройки системы (docs/SETTINGS.md). Значений по умолчанию нет.

pub mod catalog;
pub mod live;
pub mod read;
pub mod schema;
pub mod store;
pub mod value;

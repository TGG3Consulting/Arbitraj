//! Файл: notify/mod.rs
//! Что делает: уведомления в Telegram — какие события слать, решает настройка каждого события.
//! Кто вызывает: main.rs, api, позже ядро и шлюз.
//! От чего зависит: settings, secrets, storage/journal. Никогда — venues.
//! Настройки: notify.* (docs/SETTINGS.md).

pub mod catalog;
pub mod check;
pub mod client;
pub mod dispatch;
pub mod settings_watch;
pub mod telegram;

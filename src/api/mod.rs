//! Файл: api/mod.rs
//! Что делает: веб-сервер для интерфейса (axum): REST и WebSocket.
//! Кто вызывает: main.rs.
//! От чего зависит: settings, storage, gateway, notify, secrets. Никогда — venues.
//! Настройки: нет своих; адрес — аргумент запуска --listen.

pub mod check;
pub mod dex;
pub mod guard;
pub mod journal;
pub mod rules;
pub mod rules_meta;
pub mod secrets;
pub mod server;
pub mod settings;
pub mod state;
pub mod static_ui;
pub mod venues;
pub mod ws;

//! Файл: api/mod.rs
//! Что делает: веб-сервер для интерфейса (axum): REST и WebSocket.
//! Кто вызывает: main.rs.
//! От чего зависит: settings, storage, gateway, notify, secrets. Никогда — venues.
//! Настройки: нет своих; адрес — аргумент запуска --listen.

pub mod balances;
pub mod check;
pub mod coins;
pub mod control;
pub mod core_api;
pub mod dex;
pub mod dex_dry;
pub mod dex_executor;
pub mod episodes;
pub mod first_run;
pub mod guard;
pub mod journal;
pub mod rules;
pub mod rules_codes;
pub mod rules_meta;
pub mod secrets;
pub mod server;
pub mod settings;
pub mod settings_io;
pub mod settings_io_fmt;
pub mod state;
pub mod static_ui;
pub mod uncapped;
pub mod usdt_transfer;
pub mod venues;
pub mod wallet_queue;
pub mod wallets;
pub mod wallets_dex;
pub mod wallets_migrate;
pub mod wallets_view;
pub mod windows;
pub mod ws;

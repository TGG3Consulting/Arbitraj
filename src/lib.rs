//! Файл: lib.rs
//! Что делает: библиотека системы — все модули. Программа (main.rs) только собирает и запускает их.
//! Кто вызывает: main.rs, тесты.
//! От чего зависит: все модули.
//! Настройки: нет.
//!
//! Слои: core (ядро) → gateway (шлюз) → venues (адаптеры, появятся на этапе 2).
//! Ядро не импортирует venues — только gateway (CLAUDE.md).

pub mod api;
pub mod cli;
pub mod core;
pub mod gateway;
pub mod notify;
pub mod secrets;
pub mod settings;
pub mod storage;
pub mod venues;

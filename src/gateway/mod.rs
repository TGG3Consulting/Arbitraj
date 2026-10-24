//! Файл: gateway/mod.rs
//! Что делает: шлюз — единственный путь между ядром и площадками.
//! Контракт адаптера, единые ошибки, события, правила при сбоях, реестр адаптеров.
//! Кто вызывает: ядро, API, main.rs.
//! От чего зависит: core/types. Никогда — venues (адаптеры регистрируются из main.rs).
//! Настройки: правила шлюза (хранятся в SQLite, меняются из интерфейса).

pub mod contract;
pub mod errors;
pub mod events;
pub mod registry;
pub mod rules;
pub mod urgency;

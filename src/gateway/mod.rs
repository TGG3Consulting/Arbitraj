//! Файл: gateway/mod.rs
//! Что делает: шлюз — единственный путь между ядром и площадками.
//! Контракт адаптера, единые ошибки, события, правила при сбоях, реестр адаптеров.
//! Кто вызывает: ядро, API, main.rs.
//! От чего зависит: core/types. Никогда — venues (адаптеры регистрируются из main.rs).
//! Настройки: правила шлюза (хранятся в SQLite, меняются из интерфейса).

pub mod calc_meter;
pub mod contract;
pub mod errors;
pub mod events;
pub mod feed_health;
pub mod feed_reg;
pub mod frozen;
pub mod http;
pub mod http_bind;
pub mod network_hint;
pub mod patience;
pub mod pending;
pub mod quote_stage;
pub mod registry;
pub mod rules;
pub mod tap;
pub mod tx_hash;
pub mod urgency;

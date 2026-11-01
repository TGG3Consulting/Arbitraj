//! Файл: venues/mod.rs
//! Что делает: адаптеры площадок. Каждый реализует контракт gateway/contract.rs.
//! Кто вызывает: main.rs (создаёт и регистрирует в реестре шлюза). Ядро — никогда.
//! От чего зависит: gateway, settings, secrets, storage.
//! Настройки: venue.* по каждой площадке.

pub mod bingx;
pub mod bsc_dex;
pub mod kyberswap;

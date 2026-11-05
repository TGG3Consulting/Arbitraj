//! Файл: venues/mod.rs
//! Что делает: адаптеры площадок. Каждый реализует контракт gateway/contract.rs.
//! Кто вызывает: main.rs (создаёт и регистрирует в реестре шлюза). Ядро — никогда.
//! От чего зависит: gateway, settings, secrets, storage.
//! Настройки: venue.* по каждой площадке.

pub mod binance;
pub mod bingx;
pub mod bitget;
pub mod bsc_dex;
pub mod bybit;
pub mod gate;
pub mod kucoin;
pub mod kyberswap;
pub mod mexc;
pub mod okx;
pub mod phemex;
pub mod poloniex;
pub mod xt;

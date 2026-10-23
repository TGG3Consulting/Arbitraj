//! Файл: core/types/mod.rs
//! Что делает: общие типы ядра — единый язык данных между ядром, шлюзом и адаптерами.
//! Кто вызывает: весь проект.
//! От чего зависит: market.rs, trade.rs, transfer.rs.
//! Настройки: нет.
//!
//! Это стык: менять только через запрос на изменение стыка (docs/AGENTS.md).

pub mod market;
pub mod trade;
pub mod transfer;

pub use market::{Asset, Direction, OrderBook, Pool, PoolState, PriceLevel, VenueId};
pub use trade::{Quote, Side, TradeRequest, TradeResult, TradeStatus};
pub use transfer::{Balance, CoinNetworkInfo, Transfer, TransferStatus, WithdrawRequest};

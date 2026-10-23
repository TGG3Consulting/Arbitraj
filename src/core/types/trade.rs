//! Файл: core/types/trade.rs
//! Что делает: типы торговли — сторона сделки, котировка на объём, запрос и результат покупки/продажи.
//! Кто вызывает: ядро (расчёт, исполнение), шлюз и адаптеры через контракт.
//! От чего зависит: core/types/market.rs.
//! Настройки: нет.
//!
//! Покупка и продажа описываются одинаково для биржи и DEX:
//! у биржи это лимитный ордер, у DEX — своп с минимально допустимым результатом.

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use super::market::{Asset, VenueId};

/// Сторона сделки.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Side {
    Buy,
    Sell,
}

/// Котировка на объём: сколько заплатим или получим за заданное количество монет
/// с учётом реальной ликвидности (стакан — по уровням, пул — по его содержимому).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Quote {
    pub venue: VenueId,
    pub asset: Asset,
    pub side: Side,
    /// Запрошенное количество монет.
    pub qty: Decimal,
    /// Количество, которое площадка реально может исполнить (меньше qty, если не хватает ликвидности).
    pub fillable_qty: Decimal,
    /// Средняя цена исполнения в USDT.
    pub avg_price: Decimal,
    /// Торговая комиссия площадки (у DEX — комиссия пула) в USDT.
    pub fee_usdt: Decimal,
    /// Время, на которое рассчитана котировка.
    pub at: DateTime<Utc>,
}

/// Запрос на покупку или продажу.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TradeRequest {
    pub venue: VenueId,
    pub asset: Asset,
    pub side: Side,
    /// Количество монет.
    pub qty: Decimal,
    /// Худшая допустимая средняя цена: для биржи — цена лимитного ордера,
    /// для DEX — из неё считается минимально допустимый результат свопа.
    pub worst_price: Decimal,
    /// Идентификатор сделки ядра, для связи с журналом.
    pub deal_id: String,
}

/// Итоговое состояние исполнения.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TradeStatus {
    Filled,
    PartiallyFilled,
    NotFilled,
}

/// Фактический результат покупки или продажи.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TradeResult {
    pub request: TradeRequest,
    pub status: TradeStatus,
    pub filled_qty: Decimal,
    pub avg_price: Decimal,
    /// Фактическая комиссия.
    pub fee: Decimal,
    /// В чём взята комиссия (монета, USDT, BNB за газ).
    pub fee_asset: String,
    /// Идентификатор ордера на бирже или хеш транзакции свопа.
    pub venue_ref: String,
    pub finished_at: DateTime<Utc>,
}

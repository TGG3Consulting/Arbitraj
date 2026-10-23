//! Файл: core/types/market.rs
//! Что делает: типы рыночных данных — площадка, монета, направление, стакан.
//! Пулы DEX — в core/types/pool.rs.
//! Кто вызывает: весь проект (ядро, шлюз, адаптеры через контракт).
//! От чего зависит: rust_decimal (цены и объёмы), alloy-primitives (адрес контракта).
//! Настройки: нет.

use alloy_primitives::Address;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// Идентификатор площадки (биржа или DEX), например «bingx» или «pancakeswap».
/// Значение задаёт адаптер площадки; ядро сравнивает только на равенство.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct VenueId(pub String);

/// Монета: тикер и адрес контракта токена в сети BSC.
/// Адрес — главный идентификатор: одинаковый тикер на разных площадках может быть разными токенами.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Asset {
    /// Тикер, как его показывает площадка (например «CAKE»).
    pub ticker: String,
    /// Адрес контракта в BSC; None — площадка не сообщила адрес.
    pub bsc_contract: Option<Address>,
}

/// Направление: монета, где покупаем (A) и где продаём (B).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Direction {
    pub asset: Asset,
    /// Площадка покупки (роль A).
    pub buy_on: VenueId,
    /// Площадка продажи (роль B).
    pub sell_on: VenueId,
}

/// Один уровень стакана: цена и количество монет на ней.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PriceLevel {
    pub price: Decimal,
    pub qty: Decimal,
}

/// Стакан биржи по одной монете.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderBook {
    pub venue: VenueId,
    pub asset: Asset,
    /// Покупатели, от лучшей цены к худшей.
    pub bids: Vec<PriceLevel>,
    /// Продавцы, от лучшей цены к худшей.
    pub asks: Vec<PriceLevel>,
    /// Время площадки; None — площадка его не прислала.
    pub exchange_time: Option<DateTime<Utc>>,
    /// Время получения на нашем сервере.
    pub received_at: DateTime<Utc>,
}

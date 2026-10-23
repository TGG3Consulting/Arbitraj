//! Файл: core/types/market.rs
//! Что делает: типы рыночных данных — площадка, монета, направление, стакан, состояние пула.
//! Кто вызывает: весь проект (ядро, шлюз, адаптеры через контракт).
//! От чего зависит: rust_decimal (цены и объёмы), alloy-primitives (адреса и числа блокчейна).
//! Настройки: нет.

use alloy_primitives::{Address, U256};
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

/// Состояние пула DEX. Цену и котировку на объём считает адаптер DEX.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PoolState {
    /// Пул V2: остатки двух токенов.
    V2 { reserve0: U256, reserve1: U256 },
    /// Пул V3: текущая цена в формате протокола, ликвидность и тик.
    V3 {
        sqrt_price_x96: U256,
        liquidity: u128,
        tick: i32,
    },
}

/// Пул DEX с его текущим состоянием.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pool {
    /// Площадка (конкретная DEX).
    pub venue: VenueId,
    /// Адрес контракта пула.
    pub address: Address,
    pub token0: Address,
    pub token1: Address,
    pub state: PoolState,
    /// Номер блока, в котором получено состояние.
    pub block: u64,
    /// Время получения на нашем сервере.
    pub received_at: DateTime<Utc>,
}

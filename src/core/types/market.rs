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
/// Решение 211 (Тигран, 07.11.2026: «метчить монеты только по адресам»): монета — это АДРЕС. Равенство и хэш — по
/// адресу контракта, если он есть; тикер в них не участвует (у разных площадок тикер одного адреса бывает разным:
/// bingx NMT = gate NETMIND). Монета без адреса (площадка не сообщила; USDT/BNB по настройкам) — по тикеру.
/// `ticker` у монеты ядра — показное имя (core/registry_build.rs), у монеты в вызове площадки — тикер этой площадки
/// (core/venue_ticker.rs: on_venue). Формат JSON не изменился.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Asset {
    /// Тикер, как его показывает площадка (например «CAKE»); у монеты ядра — показное имя.
    pub ticker: String,
    /// Адрес контракта в BSC; None — площадка не сообщила адрес.
    pub bsc_contract: Option<Address>,
}

impl PartialEq for Asset {
    /// Решение 211: по адресу, если он есть у обеих; адрес только у одной — разные монеты; у обеих нет — по тикеру.
    fn eq(&self, other: &Self) -> bool {
        match (self.bsc_contract, other.bsc_contract) {
            (Some(a), Some(b)) => a == b,
            (None, None) => self.ticker == other.ticker,
            _ => false,
        }
    }
}

impl Eq for Asset {}

impl std::hash::Hash for Asset {
    /// Решение 211: согласовано с равенством — адрес, а без адреса — тикер.
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        match self.bsc_contract {
            Some(a) => {
                0u8.hash(state);
                a.hash(state);
            }
            None => {
                1u8.hash(state);
                self.ticker.hash(state);
            }
        }
    }
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

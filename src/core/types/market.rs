//! Файл: core/types/market.rs
//! Что делает: типы рыночных данных — площадка, монета, направление, стакан.
//! Пулы DEX — в core/types/pool.rs.
//! Кто вызывает: весь проект (ядро, шлюз, адаптеры через контракт).
//! От чего зависит: rust_decimal (цены и объёмы), alloy-primitives (адрес контракта), chain.rs (сеть монеты, решение 416).
//! Настройки: нет.

use alloy_primitives::Address;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use super::chain::ChainId;

/// Идентификатор площадки (биржа или DEX), например «bingx» или «pancakeswap».
/// Значение задаёт адаптер площадки; ядро сравнивает только на равенство.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct VenueId(pub String);

/// Монета: тикер, сеть и адрес контракта токена в этой сети.
/// Решение 211 (Тигран, 07.11.2026: «метчить монеты только по адресам»): монета — это АДРЕС. Равенство и хэш — по
/// адресу контракта, если он есть; тикер в них не участвует (у разных площадок тикер одного адреса бывает разным:
/// bingx NMT = gate NETMIND). Монета без адреса (площадка не сообщила; USDT/BNB по настройкам) — по тикеру.
/// Решение 416 (шаг 1 двух сетей): у монеты есть сеть (`chain`, id из настроек — dex.chain площадки DEX). Равенство и
/// хэш — (сеть, адрес), у монеты без адреса — (сеть, тикер): одинаковый адрес в BSC и в Arbitrum — разные монеты.
/// `chain = None` — сеть не указана: монета биржи (остаток на бирже один на все сети) или запись учёта до решения 416;
/// такая монета принадлежит сети единственной площадки DEX (core/chain.rs). В шаге 1 рабочий код сеть не ставит
/// (None у всех монет) — равенство, ключи и JSON как раньше.
/// `ticker` у монеты ядра — показное имя (core/registry_build.rs), у монеты в вызове площадки — тикер этой площадки
/// (core/venue_ticker.rs: on_venue). Формат JSON: поле адреса — по-прежнему «bsc_contract»; «chain» пишется, только
/// если сеть указана — запись без сети байт в байт как раньше; старая запись без «chain» читается с None.
#[derive(Clone, Serialize, Deserialize)]
pub struct Asset {
    /// Тикер, как его показывает площадка (например «CAKE»); у монеты ядра — показное имя.
    pub ticker: String,
    /// Сеть монеты (решение 416); None — не указана (монета биржи, старая запись) — сеть площадки DEX (core/chain.rs).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chain: Option<ChainId>,
    /// Адрес контракта в сети `chain`; None — площадка не сообщила адрес. Имя в JSON — «bsc_contract» (записи учёта).
    #[serde(rename = "bsc_contract")]
    pub contract: Option<Address>,
}

impl PartialEq for Asset {
    /// Решение 211: по адресу, если он есть у обеих; адрес только у одной — разные монеты; у обеих нет — по тикеру.
    /// Решение 416: и сеть та же (None = None — как раньше).
    fn eq(&self, other: &Self) -> bool {
        if self.chain != other.chain {
            return false;
        }
        match (self.contract, other.contract) {
            (Some(a), Some(b)) => a == b,
            (None, None) => self.ticker == other.ticker,
            _ => false,
        }
    }
}

impl Eq for Asset {}

impl std::hash::Hash for Asset {
    /// Решение 211: согласовано с равенством — адрес, а без адреса — тикер. Решение 416: и сеть.
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.chain.hash(state);
        match self.contract {
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

impl std::fmt::Debug for Asset {
    /// Решение 416: вид для журнала прежний — «Asset { ticker: …, bsc_contract: … }»; сеть — только если указана.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut d = f.debug_struct("Asset");
        d.field("ticker", &self.ticker);
        if let Some(c) = &self.chain {
            d.field("chain", c);
        }
        d.field("bsc_contract", &self.contract);
        d.finish()
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

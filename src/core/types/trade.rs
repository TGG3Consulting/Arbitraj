//! Файл: core/types/trade.rs
//! Что делает: типы торговли — сторона сделки, котировка на объём, запрос и результат покупки/продажи.
//! Кто вызывает: ядро (расчёт, исполнение), шлюз и адаптеры через контракт.
//! От чего зависит: core/types/market.rs, cost.rs (затраты, маршрут DEX), send.rs (транзакция в сети).
//! Настройки: нет.
//!
//! Покупка и продажа описываются одинаково для биржи и DEX:
//! у биржи это лимитный ордер, у DEX — своп с минимально допустимым результатом.

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use super::cost::{Cost, DexRoute};
use super::market::{Asset, VenueId};
use super::send::ChainTx;

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
    /// Средняя цена исполнения в USDT. Проскальзывание уже внутри (уровни стакана / математика пула);
    /// у DEX внутри и всё, что помечено в costs как in_price (комиссия пула, налог, влияние крючка).
    pub avg_price: Decimal,
    /// Затраты этой стороны сделки, известные площадке (CostKind): торговая комиссия, комиссия пула,
    /// газ, approve, чаевые канала, налоги, влияние крючка, остаток от округления.
    pub costs: Vec<Cost>,
    /// DEX: маршрут, на который посчитана котировка (прямой пул или умный). Биржа — None.
    pub route: Option<DexRoute>,
    /// DEX, граф-режим (решения 80, 84): лучший НАЙДЕННЫЙ маршрут, если он лучше того, что идёт на сделку (`route`),
    /// но неисполним как есть (мульти-роутер) — для карточки сигнала. None — лучший найденный и есть `route`.
    #[serde(default)]
    pub signal_route: Option<DexRoute>,
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
    /// DEX: маршрут из котировки — адаптер перепроверяет его (eth_call) перед отправкой. Биржа — None.
    pub route: Option<DexRoute>,
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
    /// Фактическая комиссия (у DEX — газ в BNB, подробности в chain).
    pub fee: Decimal,
    /// В чём взята комиссия (монета, USDT, BNB за газ).
    pub fee_asset: String,
    /// Идентификатор ордера на бирже или хеш транзакции свопа.
    pub venue_ref: String,
    /// DEX: транзакция в сети (каналы, блок, газ). Биржа — None.
    pub chain: Option<ChainTx>,
    pub finished_at: DateTime<Utc>,
}

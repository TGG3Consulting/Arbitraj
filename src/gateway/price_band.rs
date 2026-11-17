//! Файл: gateway/price_band.rs
//! Что делает: решение 392 (задача t180) — ценовой коридор лимитного ордера биржи в формате ядра. Многие биржи
//! отклоняют ВЕСЬ лимитный ордер, если его цена дальше заданной доли от опорной цены (последняя сделка, средняя цена,
//! референсная цена — у каждой биржи своя, по её документации). Здесь — общий формат и общая формула, без знаний о
//! конкретной бирже:
//! - PriceBandRule — правило из справочника пар биржи (множители опорной цены по сторонам, постоянные границы или
//!   «границы считает сама биржа»), кладётся в SpotPairState.price_band;
//! - PriceBand — границы цены ордера сейчас (абсолютные числа), их отдаёт адаптер контрактом price_band;
//! - miss — где цена ордера относительно коридора: внутри; «дальше» (покупка выше верхней границы, продажа ниже
//!   нижней — меньший объём даёт цену ближе, подбор объёма может уменьшить объём); «не с той стороны» (покупка ниже
//!   нижней, продажа выше верхней — уменьшение объёма не поможет).
//!
//! Кто вызывает: адаптеры бирж (symbols_parse.rs — правило, mod.rs — контракт price_band), core/price_band.rs (учёт в
//! расчёте), gateway/events.rs (SpotPairState).
//! От чего зависит: core/types (Side).
//! Настройки: нет (числа — только из ответов API бирж).

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::core::types::Side;

/// Правило коридора пары из справочника биржи. Цена ордера покупки должна быть ≤ опорная × buy_up и ≥ опорная ×
/// buy_down, продажи — ≤ опорная × sell_up и ≥ опорная × sell_down; None у множителя — с этой стороны границы нет.
/// buy_max / sell_min — постоянные границы прямо из справочника (не от опорной цены).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PriceBandRule {
    pub buy_up: Option<Decimal>,
    pub buy_down: Option<Decimal>,
    pub sell_up: Option<Decimal>,
    pub sell_down: Option<Decimal>,
    /// Постоянная верхняя граница покупки и нижняя граница продажи (у Poloniex — highestBid / lowestAsk).
    pub buy_max: Option<Decimal>,
    pub sell_min: Option<Decimal>,
    /// От какой цены считается и откуда берётся (текст по документации биржи — для журнала и экрана).
    pub reference: String,
}

/// Границы цены лимитного ордера в момент чтения. None — с этой стороны границы нет.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PriceBand {
    pub buy_max: Option<Decimal>,
    pub buy_min: Option<Decimal>,
    pub sell_max: Option<Decimal>,
    pub sell_min: Option<Decimal>,
    /// Откуда границы — для журнала («MEXC lastPrice 0.0012 (GET /api/v3/ticker/price)» и т. п.).
    pub note: String,
}

/// Где цена ордера относительно коридора (miss). Число — нарушенная граница.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Miss {
    /// Покупка выше верхней границы или продажа ниже нижней: меньший объём даёт цену ближе — может подойти.
    Far(Decimal),
    /// Покупка ниже нижней границы или продажа выше верхней: уменьшение объёма не поможет.
    Wrong(Decimal),
}

impl PriceBandRule {
    /// Правило без единой границы (все множители и постоянные границы пусты). Ошибок нет.
    pub fn is_empty(&self) -> bool {
        [
            self.buy_up,
            self.buy_down,
            self.sell_up,
            self.sell_down,
            self.buy_max,
            self.sell_min,
        ]
        .iter()
        .all(Option::is_none)
    }

    /// Нужна ли опорная цена (есть хоть один множитель). Ошибок нет.
    pub fn needs_reference(&self) -> bool {
        [self.buy_up, self.buy_down, self.sell_up, self.sell_down]
            .iter()
            .any(Option::is_some)
    }

    /// Что делает: границы сейчас по опорной цене (опорная × множитель; постоянные границы — как есть, строже из двух).
    /// Граница не больше нуля (множитель вида 1 − x при x ≥ 1) — границы с этой стороны нет. Принимает: опорную цену
    /// (None — опорной нет: только постоянные границы), текст для журнала. Возвращает: PriceBand. Ошибок нет.
    pub fn at(&self, reference: Option<Decimal>, note: String) -> PriceBand {
        let mul = |m: Option<Decimal>| {
            reference
                .zip(m)
                .map(|(r, m)| r * m)
                .filter(|v| *v > Decimal::ZERO)
        };
        PriceBand {
            buy_max: tighter_max(mul(self.buy_up), self.buy_max),
            buy_min: mul(self.buy_down),
            sell_max: mul(self.sell_up),
            sell_min: tighter_min(mul(self.sell_down), self.sell_min),
            note,
        }
    }
}

impl PriceBand {
    /// Что делает: где цена ордера стороны side относительно коридора. Границы включительные (документация бирж:
    /// «<=» / «>=»). Принимает: сторону, цену ордера. Возвращает: None — внутри; Some(Miss). Ошибок нет.
    pub fn miss(&self, side: Side, price: Decimal) -> Option<Miss> {
        let (min, max) = match side {
            Side::Buy => (self.buy_min, self.buy_max),
            Side::Sell => (self.sell_min, self.sell_max),
        };
        let above = max.filter(|m| price > *m);
        let below = min.filter(|m| price < *m);
        match side {
            Side::Buy => above.map(Miss::Far).or(below.map(Miss::Wrong)),
            Side::Sell => below.map(Miss::Far).or(above.map(Miss::Wrong)),
        }
    }

    /// Коридор без единой границы. Ошибок нет.
    pub fn is_empty(&self) -> bool {
        [self.buy_max, self.buy_min, self.sell_max, self.sell_min]
            .iter()
            .all(Option::is_none)
    }
}

/// Меньшая из двух верхних границ (None — границы нет). Ошибок нет.
pub fn tighter_max(a: Option<Decimal>, b: Option<Decimal>) -> Option<Decimal> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

/// Большая из двух нижних границ (None — границы нет). Ошибок нет.
pub fn tighter_min(a: Option<Decimal>, b: Option<Decimal>) -> Option<Decimal> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (a, b) => a.or(b),
    }
}

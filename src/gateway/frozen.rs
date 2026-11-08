//! Файл: gateway/frozen.rs
//! Что делает: решение 217 — «замороженный» источник котировок стороны сделки. Снимок берётся в момент решения о
//! входе (до отправки ордера покупки) из того, что адаптер уже держит в памяти: у биржи — копия её стакана (тот же,
//! по которому она котирует), у DEX — состояния пулов за `Arc` (копия при записи). Потом по снимку можно считать
//! котировку на любой объём — расчёт «без потолка» по сделке идёт фоном на рынке момента решения, а не на рынке, в
//! котором наша же покупка уже съела стакан или сдвинула пул.
//! Ядро не знает, что внутри снимка: это непрозрачный хэндл (`QuoteSnapshot`), котировка — в формате ядра (Quote).
//! Стакан биржи (`FrozenBook`) — общий вид снимка для всех бирж: проход по уровням и комиссия тейкера по плану,
//! который задаёт адаптер (`BookFee`: ставка, от чего считается, в какой монете). Живая котировка бирж считается
//! этим же кодом (свой снимок на один вызов) — у живой и замороженной котировки одна формула.
//! Кто вызывает: контракт адаптера (contract.rs quote_snapshot), адаптеры бирж (venues/*/book.rs), адаптер DEX
//! (venues/bsc_dex/frozen.rs), ядро (core/frozen.rs, costs_quote.rs).
//! От чего зависит: core/types (OrderBook, Quote, Cost), errors.rs.
//! Настройки: нет.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;

use crate::core::types::{Asset, Cost, CostKind, OrderBook, PriceLevel, Quote, Side, VenueId};

use super::errors::GatewayResult;

/// Снимок котировки стороны сделки на момент решения.
#[async_trait]
pub trait FrozenQuote: Send + Sync {
    /// Что делает: котировка на объём по данным снимка (не по живым).
    /// Принимает: количество монет. Возвращает: Quote (fillable_qty меньше qty, если глубины снимка не хватает).
    /// Ошибки: как у котировки площадки (нет маршрута, настройка, узел для цены газа у DEX).
    async fn quote(&self, qty: Decimal) -> GatewayResult<Quote>;

    /// Стакан снимка (биржа) — для худшей цены ордера и изломов кривой в ядре; у DEX — None. Ошибок нет.
    fn book(&self) -> Option<&OrderBook>;

    /// Когда снят. Ошибок нет.
    fn taken_at(&self) -> DateTime<Utc>;

    /// Что снято — для текста записи («стакан bybit MCRT, 40 уровней», «пулы bsc_dex: 12, блок N»). Ошибок нет.
    fn describe(&self) -> String;
}

/// Хэндл снимка: дёшево клонируется, живёт до конца фонового расчёта.
pub type QuoteSnapshot = Arc<dyn FrozenQuote>;

/// План комиссии тейкера биржи для котировки по стакану (задаёт адаптер по своей документации).
#[derive(Debug, Clone)]
pub struct BookFee {
    /// Ставка (доля, не %).
    pub rate: Decimal,
    /// true — от исполненного количества монет (комиссия в монете), false — от суммы сделки.
    pub per_coin: bool,
    /// Монета комиссии (тикер монеты или котируемой монеты).
    pub asset: String,
    /// Решение 219: комиссия берётся монетой скидки (GT, BNB), а сумма — в `asset` (USDT-эквивалент); см.
    /// core/types/cost.rs Cost::paid_in. None — списывается сама `asset`.
    pub paid_in: Option<String>,
}

/// Снимок стакана биржи: копия стакана адаптера на момент снимка и план комиссии.
#[derive(Debug, Clone)]
pub struct FrozenBook {
    pub venue: VenueId,
    /// Монета в виде площадки (тикер площадки).
    pub asset: Asset,
    pub side: Side,
    pub book: OrderBook,
    pub fee: BookFee,
    pub taken_at: DateTime<Utc>,
}

impl FrozenBook {
    /// Что делает: котировка на объём по уровням стакана снимка, комиссия тейкера по плану (не входит в avg_price).
    /// Принимает: количество. Возвращает: Quote. Ошибок нет.
    pub fn price(&self, qty: Decimal) -> Quote {
        let levels = match self.side {
            Side::Buy => &self.book.asks,
            Side::Sell => &self.book.bids,
        };
        let (filled, notional) = walk(levels, qty);
        let avg = if filled.is_zero() {
            Decimal::ZERO
        } else {
            notional / filled
        };
        let base = if self.fee.per_coin { filled } else { notional };
        Quote {
            venue: self.venue.clone(),
            asset: self.asset.clone(),
            side: self.side,
            qty,
            fillable_qty: filled,
            avg_price: avg,
            costs: vec![{
                let c = Cost::new(
                    CostKind::TradeFee,
                    base * self.fee.rate,
                    &self.fee.asset,
                    false,
                );
                match &self.fee.paid_in {
                    Some(coin) => c.paid_with(coin.clone()),
                    None => c,
                }
            }],
            route: None,
            signal_route: None,
            pick: None,
            at: self.book.received_at,
        }
    }
}

#[async_trait]
impl FrozenQuote for FrozenBook {
    /// Котировка по стакану снимка. Ошибок нет.
    async fn quote(&self, qty: Decimal) -> GatewayResult<Quote> {
        Ok(self.price(qty))
    }

    /// Стакан снимка. Ошибок нет.
    fn book(&self) -> Option<&OrderBook> {
        Some(&self.book)
    }

    /// Когда снят. Ошибок нет.
    fn taken_at(&self) -> DateTime<Utc> {
        self.taken_at
    }

    /// Текст: площадка, монета, уровни стороны, время стакана. Ошибок нет.
    fn describe(&self) -> String {
        let n = match self.side {
            Side::Buy => self.book.asks.len(),
            Side::Sell => self.book.bids.len(),
        };
        format!(
            "стакан {} {}: {n} уровней, получен {}",
            self.venue.0,
            self.asset.ticker,
            self.book.received_at.format("%H:%M:%S%.3f")
        )
    }
}

/// Проход по уровням от лучшего: сколько монет набрано и на какую сумму. Уровни отсортированы от лучшей цены к
/// худшей (разбор каждого адаптера). Принимает: уровни, количество. Возвращает: (набрано, сумма). Ошибок нет.
pub fn walk(levels: &[PriceLevel], qty: Decimal) -> (Decimal, Decimal) {
    let (mut left, mut filled, mut notional) = (qty, Decimal::ZERO, Decimal::ZERO);
    for l in levels {
        if left <= Decimal::ZERO {
            break;
        }
        let take = left.min(l.qty);
        filled += take;
        notional += take * l.price;
        left -= take;
    }
    (filled, notional)
}

/// Цена самого глубокого уровня, до которого дойдёт объём (худшая цена лимитного ордера, как
/// core/market_read.rs deepest_price). Принимает: стакан, сторону, количество. Возвращает: цену; None — глубины не
/// хватает. Ошибок нет.
pub fn deepest(book: &OrderBook, side: Side, qty: Decimal) -> Option<Decimal> {
    let levels = match side {
        Side::Buy => &book.asks,
        Side::Sell => &book.bids,
    };
    let mut left = qty;
    for l in levels {
        left -= l.qty;
        if left <= Decimal::ZERO {
            return Some(l.price);
        }
    }
    None
}

//! Файл: gateway/rest_book.rs
//! Что делает: решение 238 — общий вид стакана из ответа REST биржи (контракт quote_rest): уровни, разобранные
//! адаптером по документации своей биржи, приводятся к стакану ядра одинаково у всех — нулевой объём — уровня нет,
//! покупатели по убыванию цены, продавцы по возрастанию (порядок уровней в ответе документации бирж описывают не все;
//! сортировка не меняет его, если он уже от лучшей цены), received_at — момент разбора ответа (часы бота).
//! Кто вызывает: адаптеры бирж (venues/*: snapshot стакана по REST).
//! От чего зависит: core/types (OrderBook, PriceLevel).
//! Настройки: нет.

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;

use crate::core::types::{Asset, OrderBook, PriceLevel, VenueId};

/// Стакан из уровней ответа REST. Принимает: площадку, монету (тикер площадки), уровни покупателей и продавцов (в любом
/// порядке, с нулевыми), время биржи из ответа (None — в ответе нет). Возвращает: OrderBook. Ошибок нет.
pub fn book(
    venue: VenueId,
    asset: Asset,
    bids: Vec<PriceLevel>,
    asks: Vec<PriceLevel>,
    exchange_time: Option<DateTime<Utc>>,
) -> OrderBook {
    let keep = |l: Vec<PriceLevel>| -> Vec<PriceLevel> {
        l.into_iter().filter(|l| l.qty > Decimal::ZERO).collect()
    };
    let mut bids = keep(bids);
    let mut asks = keep(asks);
    bids.sort_by_key(|l| std::cmp::Reverse(l.price));
    asks.sort_by_key(|l| l.price);
    OrderBook {
        venue,
        asset,
        bids,
        asks,
        exchange_time,
        received_at: Utc::now(),
    }
}

/// Время биржи в миллисекундах unix → DateTime. None — нет или вне диапазона. Ошибок нет.
pub fn ms(v: Option<i64>) -> Option<DateTime<Utc>> {
    v.and_then(DateTime::<Utc>::from_timestamp_millis)
}

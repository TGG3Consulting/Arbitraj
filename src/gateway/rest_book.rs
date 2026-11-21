//! Файл: gateway/rest_book.rs
//! Что делает: решение 238 — общий вид стакана из ответа REST биржи (контракт quote_rest): уровни, разобранные
//! адаптером по документации своей биржи, приводятся к стакану ядра одинаково у всех — нулевой объём — уровня нет,
//! покупатели по убыванию цены, продавцы по возрастанию (порядок уровней в ответе документации бирж описывают не все;
//! сортировка не меняет его, если он уже от лучшей цены), received_at — момент разбора ответа (часы бота).
//! t79: from_print — стакан тихой пары из уровней ответа REST сторожа (контракт book_from_rest).
//! Кто вызывает: адаптеры бирж (venues/*: snapshot стакана по REST; mexc, bingx, kucoin book_rest.rs — from_print).
//! От чего зависит: core/types (OrderBook, PriceLevel), gateway/feed_health.rs (BookPrint).
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

/// t79 (Тигран 10.11: согласовано): стакан пары из уровней ответа REST, по которому сторож ядра признал тихую пару
/// «книга не менялась с переподписки» (core/feed_rest_ref.rs RestBook; контракт book_from_rest). Уровни — как дал
/// отпечаток ответа (первые N от лучшей цены, нулевых нет — book_print.rs адаптера), received_at — получение ответа
/// (recv_ms, часы бота), а не момент постановки: свежесть стакана считается от ответа. Времени биржи нет (None).
/// Принимает: площадку, монету адаптера, уровни, время получения ответа (unix мс). Ошибка — текст: время вне диапазона.
pub fn from_print(
    venue: VenueId,
    asset: Asset,
    levels: &crate::gateway::feed_health::BookPrint,
    recv_ms: i64,
) -> Result<OrderBook, String> {
    let received_at = DateTime::<Utc>::from_timestamp_millis(recv_ms)
        .ok_or_else(|| format!("время ответа REST {recv_ms} мс вне диапазона"))?;
    Ok(OrderBook {
        venue,
        asset,
        bids: levels.bids.clone(),
        asks: levels.asks.clone(),
        exchange_time: None,
        received_at,
    })
}

/// Время биржи в миллисекундах unix → DateTime. None — нет или вне диапазона. Ошибок нет.
pub fn ms(v: Option<i64>) -> Option<DateTime<Utc>> {
    v.and_then(DateTime::<Utc>::from_timestamp_millis)
}

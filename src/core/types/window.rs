//! Файл: core/types/window.rs
//! Что делает: разобранный кадр записи окна (решение 125) в формате ядра — для страницы окна: стакан на момент кадра
//! или сделки кадра. Сам файл окна хранит кадры сырыми; разбирает их адаптер своей площадки (Venue::decode_tap).
//! Кто вызывает: адаптеры бирж (decode_tap), core/window_view.rs, интерфейс (JSON).
//! От чего зависит: market.rs (PriceLevel), rust_decimal.
//! Настройки: нет.

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use super::market::PriceLevel;

/// Одна сделка с ленты площадки.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TradeTick {
    /// ID сделки у площадки (у MEXC может не прийти — пусто).
    pub id: String,
    pub price: Decimal,
    pub qty: Decimal,
    /// Сторона агрессора: «buy» — активная покупка, «sell» — активная продажа (из поля площадки, см. адаптер).
    pub taker: String,
    /// Время сделки у площадки, Unix мс.
    pub time_ms: i64,
}

/// Что в кадре.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum TapView {
    /// Стакан: уровни, как их прислала площадка в этом кадре (снимок частичного стакана).
    Book {
        bids: Vec<PriceLevel>,
        asks: Vec<PriceLevel>,
    },
    /// Сделки кадра (BingX — одна, MEXC — пачка за интервал).
    Trades(Vec<TradeTick>),
    /// Изменения стакана (дополнение решения 125): уровни с НОВЫМ количеством (0 — уровня больше нет); full — это полный
    /// стакан (BingX action «all», снимок REST MEXC), с которого начинают; версии — с какой по какую (у BingX обе —
    /// lastUpdateId, у MEXC — fromVersion / toVersion, у снимка — lastUpdateId).
    Diff {
        bids: Vec<PriceLevel>,
        asks: Vec<PriceLevel>,
        full: bool,
        from: Option<i64>,
        to: Option<i64>,
    },
    /// Кадр не стакан и не сделки (ответ на подписку и т. п.) — текст для показа.
    Other(String),
}

/// Разобранный кадр: содержимое и время площадки (поле времени сообщения), Unix мс.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TapDecoded {
    pub view: TapView,
    pub exchange_ms: Option<i64>,
}

/// Итог сделок окна для отметки «плюсовая / минусовая».
pub fn outcome(sum: Option<Decimal>) -> Option<&'static str> {
    let s = sum?;
    Some(if s > Decimal::ZERO {
        "plus"
    } else if s < Decimal::ZERO {
        "minus"
    } else {
        "zero"
    })
}

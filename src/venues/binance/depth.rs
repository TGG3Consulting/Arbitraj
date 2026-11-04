//! Файл: venues/binance/depth.rs
//! Что делает: разбор кадров WebSocket рынка Binance (комбинированный поток /stream): частичный стакан
//! <symbol>@depth<N>[@100ms] → уровни; ответ на SUBSCRIBE; ошибка запроса. Уровни разбираются без копий строк
//! (заимствование из кадра), в Decimal — только когда lastUpdateId сменился (решения 109, 130, урок 13).
//! Кто вызывает: ws_conn.rs, tap_decode.rs.
//! От чего зависит: core/types, serde.
//! Настройки: нет.
//!
//! Официально (https://developers.binance.com/legacy-docs/binance-spot-api-docs/web-socket-streams):
//! - комбинированный поток: «/stream?streams=…», события в обёртке {"stream":"<streamName>","data":<rawPayload>};
//!   символы в потоках — строчные;
//! - Partial Book Depth: «<symbol>@depth<levels>» или «…@100ms», levels 5, 10, 20; data {lastUpdateId, bids [[цена,
//!   количество]], asks} — снимок; порядок уровней не оговорён — сортируем сами (asks по возрастанию, bids по убыванию);
//! - SUBSCRIBE: ответ {"result": null, "id": 1}; ошибка — {"code": N, "msg": "…", "id": …} (таблица Error Messages:
//!   коды 0–3 — все ошибки, «code» есть только у ошибки).

use std::borrow::Cow;
use std::str::FromStr;

use chrono::Utc;
use rust_decimal::Decimal;
use serde::Deserialize;

use crate::core::types::{Asset, OrderBook, PriceLevel, VenueId};

/// Уровень как в кадре: [цена, количество] строками.
type RawLevel<'a> = (Cow<'a, str>, Cow<'a, str>);

/// Снимок частичного стакана (data).
#[derive(Deserialize)]
pub struct Partial<'a> {
    #[serde(rename = "lastUpdateId")]
    pub last_update_id: i64,
    #[serde(borrow)]
    pub bids: Vec<RawLevel<'a>>,
    #[serde(borrow)]
    pub asks: Vec<RawLevel<'a>>,
}

/// Кадр целиком: данные потока, ответ или ошибка запроса.
#[derive(Deserialize)]
struct Frame<'a> {
    #[serde(borrow, default)]
    stream: Option<Cow<'a, str>>,
    #[serde(borrow, default)]
    data: Option<Partial<'a>>,
    #[serde(default)]
    id: Option<serde_json::Value>,
    #[serde(default)]
    code: Option<i64>,
    #[serde(borrow, default)]
    msg: Option<Cow<'a, str>>,
}

/// Что пришло в кадре.
pub enum Msg<'a> {
    /// Стакан: символ потока строчными (до первого «@»), снимок.
    Depth { symbol: String, book: Partial<'a> },
    /// Ответ на запрос без ошибки (id запроса).
    Ack(String),
    /// Ошибка запроса: код, текст, id.
    Error { code: i64, msg: String, id: String },
    /// Кадр не разобран — причина.
    Bad(String),
    /// Другое (только в сыром журнале).
    Other,
}

/// Разбирает текстовый кадр. Ошибок нет: неразборчивый — Msg::Bad.
pub fn parse(text: &str) -> Msg<'_> {
    let f: Frame<'_> = match serde_json::from_str(text) {
        Ok(f) => f,
        Err(e) => return Msg::Bad(format!("кадр не JSON стакана: {e}")),
    };
    let id = f.id.map(|v| v.to_string()).unwrap_or_default();
    if let Some(code) = f.code {
        let msg = f.msg.map(|m| m.into_owned()).unwrap_or_default();
        return Msg::Error { code, msg, id };
    }
    match (f.stream, f.data) {
        (Some(s), Some(book)) => Msg::Depth {
            symbol: s.split('@').next().unwrap_or_default().to_string(),
            book,
        },
        (None, None) if !id.is_empty() => Msg::Ack(id),
        _ => Msg::Other,
    }
}

/// Что делает: уровни кадра → PriceLevel с сортировкой (asks по возрастанию цены, bids по убыванию); количество 0 —
/// уровня нет. Принимает: снимок. Возвращает: (bids, asks). Ошибка — текст причины (не число).
pub fn levels(b: &Partial<'_>) -> Result<(Vec<PriceLevel>, Vec<PriceLevel>), String> {
    let mut bids = convert(&b.bids, "bids")?;
    let mut asks = convert(&b.asks, "asks")?;
    asks.sort_by_key(|a| a.price);
    bids.sort_by_key(|b| std::cmp::Reverse(b.price));
    Ok((bids, asks))
}

/// Строки → PriceLevel без нулевых. Ошибка — текст.
fn convert(items: &[RawLevel<'_>], side: &str) -> Result<Vec<PriceLevel>, String> {
    let mut out = Vec::with_capacity(items.len());
    for (p, q) in items {
        let price = Decimal::from_str(p).map_err(|_| format!("{side}: цена «{p}» не число"))?;
        let qty = Decimal::from_str(q).map_err(|_| format!("{side}: количество «{q}» не число"))?;
        if qty > Decimal::ZERO {
            out.push(PriceLevel { price, qty });
        }
    }
    Ok(out)
}

/// Собирает OrderBook ядра (времени биржи в частичном стакане нет — exchange_time пусто). Ошибок нет.
pub fn to_book(asset: Asset, bids: Vec<PriceLevel>, asks: Vec<PriceLevel>) -> OrderBook {
    OrderBook {
        venue: VenueId(super::ctx::ID.to_string()),
        asset,
        bids,
        asks,
        exchange_time: None,
        received_at: Utc::now(),
    }
}

//! Файл: venues/mexc/depth.rs
//! Что делает: разбор кадров WebSocket рынка MEXC: текстовые JSON-ответы (подписка, PONG) и двоичные кадры
//! Protocol Buffers (обёртка PushDataV3ApiWrapper) → частичный стакан канала spot@public.limit.depth.v3.api.pb →
//! OrderBook формата ядра.
//! Кто вызывает: ws_conn.rs.
//! От чего зависит: proto.rs, core/types, prost.
//! Настройки: нет.
//!
//! Официальная документация:
//! - частичный стакан https://www.mexc.com/api-docs/spot-v3/websocket-market-streams/partial-book-depth-streams —
//!   канал «spot@public.limit.depth.v3.api.pb@<symbol>@<level>», level 5, 10 или 20; каждый кадр — снимок уровней;
//! - подписка и PING https://www.mexc.com/api-docs/spot-v3/websocket-market-streams/ —
//!   {"method":"SUBSCRIPTION","params":[…]}, {"method":"PING"} → {"id":0,"code":0,"msg":"PONG"}; «If the msg in the
//!   response matches the corresponding request field, it indicates that the request was sent successfully».
//!
//! Порядок уровней не оговорён — сортируем сами: asks по возрастанию цены, bids по убыванию (как у BingX).

use std::str::FromStr;

use chrono::{DateTime, TimeZone, Utc};
use prost::Message as _;
use rust_decimal::Decimal;
use serde_json::Value;

use crate::core::types::{Asset, OrderBook, PriceLevel};

use super::proto::{Body, DepthItem, Wrapper};

/// Что пришло в кадре.
pub enum Msg {
    /// Ответ PONG на наш PING.
    Pong,
    /// Стакан пары (символ MEXC; монету подставляет вызывающий).
    Depth {
        symbol: String,
        bids: Vec<PriceLevel>,
        asks: Vec<PriceLevel>,
        exchange_time: Option<DateTime<Utc>>,
    },
    /// Текстовый ответ: code 0 — успех (msg — канал подписки); иначе — отказ с текстом.
    Ack { code: i64, msg: String },
    /// Кадр не разобран (protobuf или уровни) — символ, если известен, и причина.
    Bad {
        symbol: Option<String>,
        reason: String,
    },
    /// Другое (только в сырых сообщениях).
    Other,
}

/// Разбирает текстовый кадр (JSON). Ошибок нет.
pub fn parse_text(text: &str) -> Msg {
    let Ok(v) = serde_json::from_str::<Value>(text) else {
        return Msg::Other;
    };
    let msg = v
        .get("msg")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    match v.get("code").and_then(Value::as_i64) {
        Some(0) if msg == "PONG" => Msg::Pong,
        Some(code) => Msg::Ack { code, msg },
        None => Msg::Other,
    }
}

/// Разбирает двоичный кадр (protobuf обёртки). Ошибок нет: неразборчивый кадр — Msg::Bad.
pub fn parse_binary(data: &[u8]) -> Msg {
    let w = match Wrapper::decode(data) {
        Ok(w) => w,
        Err(e) => {
            return Msg::Bad {
                symbol: None,
                reason: format!("protobuf: {e}"),
            };
        }
    };
    let symbol = w.symbol.clone().or_else(|| symbol_from_channel(&w.channel));
    let Some(Body::LimitDepths(d)) = w.body else {
        return Msg::Other;
    };
    let Some(symbol) = symbol else {
        return Msg::Bad {
            symbol: None,
            reason: format!("стакан без символа (канал {})", w.channel),
        };
    };
    let (mut asks, mut bids) = match (levels(&d.asks, "asks"), levels(&d.bids, "bids")) {
        (Ok(a), Ok(b)) => (a, b),
        (Err(reason), _) | (_, Err(reason)) => {
            return Msg::Bad {
                symbol: Some(symbol),
                reason,
            };
        }
    };
    asks.sort_by_key(|a| a.price);
    bids.sort_by_key(|b| std::cmp::Reverse(b.price));
    let ms = w.send_time.or(w.create_time);
    Msg::Depth {
        symbol,
        bids,
        asks,
        exchange_time: ms.and_then(|ms| Utc.timestamp_millis_opt(ms).single()),
    }
}

/// Символ из канала «spot@public.limit.depth.v3.api.pb@CAKEUSDT@20» (третья часть после @). Ошибок нет.
fn symbol_from_channel(ch: &str) -> Option<String> {
    ch.split('@').nth(2).map(str::to_string)
}

/// Уровни (строки цена / количество → Decimal). Нулевое количество — уровня нет, пропускается.
/// Ошибка — текст причины.
fn levels(items: &[DepthItem], side: &str) -> Result<Vec<PriceLevel>, String> {
    let mut out = Vec::with_capacity(items.len());
    for l in items {
        let price = Decimal::from_str(l.price.trim())
            .map_err(|_| format!("{side}: цена «{}» не число", l.price))?;
        let qty = Decimal::from_str(l.quantity.trim())
            .map_err(|_| format!("{side}: количество «{}» не число", l.quantity))?;
        if qty > Decimal::ZERO {
            out.push(PriceLevel { price, qty });
        }
    }
    Ok(out)
}

/// Собирает OrderBook ядра.
pub fn to_book(
    asset: Asset,
    bids: Vec<PriceLevel>,
    asks: Vec<PriceLevel>,
    exchange_time: Option<DateTime<Utc>>,
) -> OrderBook {
    OrderBook {
        venue: crate::core::types::VenueId(super::ctx::ID.to_string()),
        asset,
        bids,
        asks,
        exchange_time,
        received_at: Utc::now(),
    }
}

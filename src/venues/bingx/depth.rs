//! Файл: venues/bingx/depth.rs
//! Что делает: разбор сообщений WebSocket BingX: распаковка GZIP, распознавание ping,
//! стакан канала <symbol>@depth<N> → OrderBook формата ядра, ответы на подписку.
//! Кто вызывает: ws.rs.
//! От чего зависит: core/types, num.rs, flate2.
//! Настройки: нет.
//!
//! Официальная документация (docs-v3, модуль «Subscribe Market Depth Data», спот):
//! push каждые 300 мс, dataType «BTC-USDT@depth50», data.bids/data.asks — массивы [цена, количество],
//! data.lastUpdateId, время сообщения — поле timestamp верхнего уровня.
//! Порядок уровней в официальном примере не совпадает с описанием (asks пришли по убыванию),
//! поэтому уровни всегда сортируются здесь: asks — по возрастанию цены, bids — по убыванию.
//! Ping спота: JSON с полем «ping» ({"ping": "...", "time": "..."}), на него отвечаем «Pong»
//! (официальный пример для спота; ccxt pro bingx.ts — тот же формат ping). Строка «Ping» — тоже ping.

use std::io::Read;

use chrono::{DateTime, TimeZone, Utc};
use serde_json::Value;

use crate::core::types::{Asset, OrderBook, PriceLevel};

use super::num;

/// Что пришло в сообщении.
pub enum Msg {
    /// Ping сервера — нужно ответить «Pong».
    Ping,
    /// Стакан пары (символ BingX и стакан без монеты — монету подставляет вызывающий).
    Depth {
        symbol: String,
        bids: Vec<PriceLevel>,
        asks: Vec<PriceLevel>,
        exchange_time: Option<DateTime<Utc>>,
    },
    /// Ответ на подписку: код 0 — успех; иначе причина. id — идентификатор нашей подписки.
    SubAck { id: String, code: i64, msg: String },
    /// Стакан пары пришёл, но разобрать его нельзя — данные этой пары недостоверны.
    BadDepth { symbol: String, reason: String },
    /// Другое (сохраняется только в сырых сообщениях).
    Other,
}

/// Распаковывает GZIP. Ошибка — текст причины.
pub fn gunzip(data: &[u8]) -> Result<String, String> {
    let mut s = String::new();
    flate2::read::GzDecoder::new(data)
        .read_to_string(&mut s)
        .map_err(|e| format!("GZIP: {e}"))?;
    Ok(s)
}

/// Разбирает текст сообщения. Ошибок не бывает: неразборчивый стакан — Msg::BadDepth, прочее — Msg::Other.
pub fn parse(text: &str) -> Msg {
    if text == "Ping" {
        return Msg::Ping;
    }
    let Ok(v) = serde_json::from_str::<Value>(text) else {
        return Msg::Other;
    };
    if v.get("ping").is_some() {
        return Msg::Ping;
    }
    let data_type = v
        .get("dataType")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if let (Some((symbol, ch)), Some(data)) = (data_type.split_once('@'), v.get("data"))
        && ch.starts_with("depth")
        && data.is_object()
    {
        let (mut asks, mut bids) = match (levels(data, "asks"), levels(data, "bids")) {
            (Ok(a), Ok(b)) => (a, b),
            (Err(reason), _) | (_, Err(reason)) => {
                return Msg::BadDepth {
                    symbol: symbol.to_string(),
                    reason,
                };
            }
        };
        asks.sort_by_key(|a| a.price);
        bids.sort_by_key(|b| std::cmp::Reverse(b.price));
        let exchange_time = v
            .get("timestamp")
            .and_then(Value::as_i64)
            .and_then(|ms| Utc.timestamp_millis_opt(ms).single());
        return Msg::Depth {
            symbol: symbol.to_string(),
            bids,
            asks,
            exchange_time,
        };
    }
    if let Some(code) = v.get("code").and_then(Value::as_i64)
        && v.get("data").is_none_or(Value::is_null)
    {
        let msg = v
            .get("msg")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let id = v
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        return Msg::SubAck { id, code, msg };
    }
    Msg::Other
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

/// Уровни [цена, количество]. Ошибка — неверный формат.
fn levels(data: &Value, side: &str) -> Result<Vec<PriceLevel>, String> {
    let arr = data
        .get(side)
        .and_then(Value::as_array)
        .ok_or(format!("нет {side}"))?;
    arr.iter()
        .map(|l| {
            let pair = l
                .as_array()
                .filter(|p| p.len() >= 2)
                .ok_or(format!("{side}: уровень не [цена, количество]"))?;
            let price = num::to_dec(&pair[0]).ok_or(format!("{side}: цена не число"))?;
            let qty = num::to_dec(&pair[1]).ok_or(format!("{side}: количество не число"))?;
            Ok(PriceLevel { price, qty })
        })
        .collect()
}

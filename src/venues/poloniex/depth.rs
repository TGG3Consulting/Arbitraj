//! Файл: venues/poloniex/depth.rs
//! Что делает: разбор текстовых кадров публичного WebSocket Poloniex: pong, события (subscribe, error) и данные канала
//! стакана book (снимки N уровней; в одном кадре может быть несколько записей) → стаканы формата ядра. Один разбор
//! JSON на кадр (урок 13).
//! Кто вызывает: ws_conn.rs, user_stream.rs (ping), тесты.
//! От чего зависит: core/types, serde_json.
//! Настройки: нет.
//!
//! Официально:
//! - Spot → WebSocket (https://api-docs.poloniex.com/spot/websocket/): ping {"event": "ping"} → {"event": "pong"};
//!   подписка принята — {"event": "subscribe", "channel": …}; ошибка — {"event": "error", "message": "…"} (символа в
//!   ошибке нет).
//! - Market Data → Book (https://api-docs.poloniex.com/spot/websocket/market-data): {"channel": "book", "data": [{symbol,
//!   createTime, asks, bids, id, ts}]}; asks «ascending», bids «descending» — уровни [цена, количество] строками; id —
//!   «Record ID (SeqId)», ts — «Send timestamp». Порядок уровней всё равно выравнивается здесь (дёшево: ≤ 20 уровней).

use std::str::FromStr;

use chrono::{DateTime, TimeZone, Utc};
use rust_decimal::Decimal;
use serde_json::Value;

use crate::core::types::PriceLevel;

/// Канал стакана снимками (официальное имя).
pub const BOOK: &str = "book";

/// Сообщение ping клиента.
pub fn ping() -> String {
    r#"{"event":"ping"}"#.to_string()
}

/// Снимок стакана одной пары.
#[derive(Debug, Clone)]
pub struct Depth {
    pub symbol: String,
    /// id записи (SeqId).
    pub seq: Option<i64>,
    pub bids: Vec<PriceLevel>,
    pub asks: Vec<PriceLevel>,
    /// Время создания записи биржей (createTime).
    pub exchange_time: Option<DateTime<Utc>>,
}

/// Что пришло в кадре.
pub enum Msg {
    Pong,
    /// Подписка принята.
    Subscribed,
    /// Ошибка сервера: текст как пришёл.
    Error(String),
    /// Снимки стаканов (записи кадра по порядку).
    Books(Vec<Depth>),
    /// Кадр стакана не разобран (символ, если известен, и причина).
    Bad {
        symbol: Option<String>,
        reason: String,
    },
    Other,
}

/// Разбирает текстовый кадр. Ошибок нет: неразборчивый — Msg::Bad или Other.
pub fn parse(text: &str) -> Msg {
    let Ok(v) = serde_json::from_str::<Value>(text) else {
        return Msg::Bad {
            symbol: None,
            reason: "кадр не JSON".into(),
        };
    };
    match v.get("event").and_then(Value::as_str) {
        Some("pong") => return Msg::Pong,
        Some("subscribe") => return Msg::Subscribed,
        Some("error") => return Msg::Error(text.to_string()),
        Some(_) => return Msg::Other,
        None => {}
    }
    if v.get("channel").and_then(Value::as_str) != Some(BOOK) {
        return Msg::Other;
    }
    let Some(items) = v.get("data").and_then(Value::as_array) else {
        return Msg::Bad {
            symbol: None,
            reason: "нет массива data".into(),
        };
    };
    let mut out = Vec::with_capacity(items.len());
    for d in items {
        let symbol = d.get("symbol").and_then(Value::as_str).unwrap_or_default();
        match depth(symbol, d) {
            Ok(x) => out.push(x),
            Err(reason) => {
                return Msg::Bad {
                    symbol: Some(symbol.to_string()).filter(|s| !s.is_empty()),
                    reason,
                };
            }
        }
    }
    Msg::Books(out)
}

/// Одна запись data. Ошибка — текст причины.
fn depth(symbol: &str, d: &Value) -> Result<Depth, String> {
    if symbol.is_empty() {
        return Err("запись без symbol".into());
    }
    let mut asks = levels(d, "asks")?;
    let mut bids = levels(d, "bids")?;
    asks.sort_by_key(|a| a.price);
    bids.sort_by_key(|b| std::cmp::Reverse(b.price));
    let int = |f: &str| match d.get(f) {
        Some(Value::String(s)) => s.parse::<i64>().ok(),
        Some(x) => x.as_i64(),
        None => None,
    };
    Ok(Depth {
        symbol: symbol.to_string(),
        seq: int("id"),
        bids,
        asks,
        exchange_time: int("createTime").and_then(|ms| Utc.timestamp_millis_opt(ms).single()),
    })
}

/// Уровни [цена, количество] (строки) → PriceLevel; нулевое количество — уровня нет. Ошибка — текст причины.
pub fn levels(d: &Value, side: &str) -> Result<Vec<PriceLevel>, String> {
    let arr = d
        .get(side)
        .and_then(Value::as_array)
        .ok_or_else(|| format!("нет {side}"))?;
    let mut out = Vec::with_capacity(arr.len());
    for l in arr {
        let p = l
            .as_array()
            .filter(|p| p.len() >= 2)
            .ok_or_else(|| format!("{side}: уровень не [цена, количество]"))?;
        let num = |x: &Value, what: &str| {
            let s = match x {
                Value::String(s) => s.trim().to_string(),
                Value::Number(n) => n.to_string(),
                _ => String::new(),
            };
            Decimal::from_str(&s).map_err(|_| format!("{side}: {what} {x} не число"))
        };
        let (price, qty) = (num(&p[0], "цена")?, num(&p[1], "количество")?);
        if qty > Decimal::ZERO {
            out.push(PriceLevel { price, qty });
        }
    }
    Ok(out)
}

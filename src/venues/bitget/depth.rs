//! Файл: venues/bitget/depth.rs
//! Что делает: разбор текстовых кадров WebSocket Bitget (Classic v2): "pong", события (subscribe, error) и данные
//! канала стакана books1/books5/books15 (action snapshot) → стакан формата ядра. Один разбор JSON на кадр (урок 13).
//! Кто вызывает: ws_conn.rs, тесты.
//! От чего зависит: core/types, serde_json.
//! Настройки: нет.
//!
//! Официально:
//! - WebSocket intro (https://www.bitget.com/docs/classic/websocket/intro): клиент шлёт строку "ping", ответ — строка
//!   "pong"; ответ на подписку {"event":"subscribe","arg":{…}}; ошибка {"event":"error","code":…,"msg":…} (пример
//!   «"code":"30005"»).
//! - Depth Channel (https://www.bitget.com/docs/classic/websocket/spot/public/Depth-Channel): кадр {"action":"snapshot",
//!   "arg":{"instType":"SPOT","channel":"books5","instId":"BTCUSDT"},"data":[{"asks":[[цена, количество]],"bids":[…],
//!   "seq":…,"ts":"…"}],"ts":…}; asks «Seller depth», bids «Buyer depth», ts «Matching engine timestamp(ms)», seq
//!   «increases when the order book is updated». books1/5/15 — «Each push is a snapshot». Порядок уровней не оговорён —
//!   сортируем сами: asks по возрастанию цены, bids по убыванию.

use std::str::FromStr;

use chrono::{DateTime, TimeZone, Utc};
use rust_decimal::Decimal;
use serde_json::Value;

use crate::core::types::PriceLevel;

/// Что пришло в кадре.
pub enum Msg {
    Pong,
    /// Подписка принята (instId из arg).
    Subscribed(String),
    /// Ошибка сервера: instId из arg (если есть) и текст как пришёл.
    Error {
        inst: Option<String>,
        text: String,
    },
    /// Снимок стакана пары: символ Bitget, номер seq, уровни, время биржи.
    Depth {
        symbol: String,
        seq: Option<i64>,
        bids: Vec<PriceLevel>,
        asks: Vec<PriceLevel>,
        exchange_time: Option<DateTime<Utc>>,
    },
    /// Кадр не разобран (символ, если известен, и причина).
    Bad {
        symbol: Option<String>,
        reason: String,
    },
    Other,
}

/// Разбирает текстовый кадр. Ошибок нет: неразборчивый — Msg::Bad или Other.
pub fn parse(text: &str) -> Msg {
    if text == "pong" {
        return Msg::Pong;
    }
    let Ok(v) = serde_json::from_str::<Value>(text) else {
        return Msg::Bad {
            symbol: None,
            reason: "кадр не JSON".into(),
        };
    };
    let inst = v
        .get("arg")
        .and_then(|a| a.get("instId"))
        .and_then(Value::as_str)
        .map(str::to_string);
    match v.get("event").and_then(Value::as_str) {
        Some("subscribe") => return Msg::Subscribed(inst.unwrap_or_default()),
        Some("error") => {
            return Msg::Error {
                inst,
                text: text.to_string(),
            };
        }
        Some(_) => return Msg::Other,
        None => {}
    }
    let channel = v
        .get("arg")
        .and_then(|a| a.get("channel"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !channel.starts_with("books") || v.get("action").and_then(Value::as_str) != Some("snapshot")
    {
        return Msg::Other;
    }
    let symbol = inst.unwrap_or_default();
    match v
        .get("data")
        .and_then(Value::as_array)
        .and_then(|a| a.first())
    {
        Some(d) => depth(symbol, d),
        None => Msg::Bad {
            symbol: Some(symbol),
            reason: "нет data[0]".into(),
        },
    }
}

/// Снимок стакана из data[0]. Ошибок нет: неразборчивый — Msg::Bad.
fn depth(symbol: String, d: &Value) -> Msg {
    let (mut asks, mut bids) = match (levels(d, "asks"), levels(d, "bids")) {
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
    let int = |f: &str| match d.get(f) {
        Some(Value::String(s)) => s.parse::<i64>().ok(),
        Some(x) => x.as_i64(),
        None => None,
    };
    Msg::Depth {
        symbol,
        seq: int("seq"),
        bids,
        asks,
        exchange_time: int("ts").and_then(|ms| Utc.timestamp_millis_opt(ms).single()),
    }
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
            x.as_str()
                .and_then(|s| Decimal::from_str(s.trim()).ok())
                .ok_or_else(|| format!("{side}: {what} {x} не число"))
        };
        let (price, qty) = (num(&p[0], "цена")?, num(&p[1], "количество")?);
        if qty > Decimal::ZERO {
            out.push(PriceLevel { price, qty });
        }
    }
    Ok(out)
}

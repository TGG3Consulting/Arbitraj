//! Файл: venues/kucoin/depth.rs
//! Что делает: разбор текстовых кадров WebSocket KuCoin: welcome, pong, ack, error и сообщения (type = message) —
//! снимок N уровней стакана (/spotMarket/level2Depth5|50), сделки (/market/match), изменения стакана (/market/level2)
//! → стакан формата ядра или сведения для окна. Один разбор JSON на кадр (урок 13).
//! Кто вызывает: ws_conn.rs, feed.rs, user_stream_conn.rs, tap.rs (разбор записанного кадра), тесты.
//! От чего зависит: core/types, serde_json.
//! Настройки: нет.
//!
//! Официально (Classic WebSocket, introduction): welcome {"id","type":"welcome"}, ping {"id","type":"ping"} →
//! pong {"id","type":"pong","timestamp"}, ack {"id","type":"ack"} при response = true; ошибки — type error.
//! Снимок Level50 https://www.kucoin.com/docs-new/3470070w0 (Level5 — 3470069w0): topic «/spotMarket/level2Depth50:SYM»,
//! subject level2, data {asks, bids — [цена, количество], timestamp}; «If there is no change in the market, data will
//! not be pushed», «once every 100ms». Живая запись: tests/data/cex_samples_2026_11_04/kucoin_ws.jsonl. Порядок уровней
//! не оговорён — сортируем сами: asks по возрастанию цены, bids по убыванию.

use std::str::FromStr;

use chrono::{DateTime, TimeZone, Utc};
use rust_decimal::Decimal;
use serde_json::Value;

use crate::core::types::PriceLevel;

/// Что пришло в кадре.
pub enum Msg {
    Welcome,
    Pong,
    /// Подтверждение подписки (id запроса).
    Ack(String),
    /// Ошибка сервера: id запроса (если есть) и текст как пришёл.
    Error {
        id: Option<String>,
        text: String,
    },
    /// Снимок стакана пары: символ KuCoin, уровни, время биржи.
    Depth {
        symbol: String,
        bids: Vec<PriceLevel>,
        asks: Vec<PriceLevel>,
        exchange_time: Option<DateTime<Utc>>,
    },
    /// Сообщение другого канала (тема и данные) — для окна и потока аккаунта.
    Data {
        topic: String,
        subject: String,
        data: Value,
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
    let Ok(mut v) = serde_json::from_str::<Value>(text) else {
        return Msg::Bad {
            symbol: None,
            reason: "кадр не JSON".into(),
        };
    };
    let id = || v.get("id").and_then(Value::as_str).map(str::to_string);
    match v.get("type").and_then(Value::as_str) {
        Some("welcome") => return Msg::Welcome,
        Some("pong") => return Msg::Pong,
        Some("ack") => return Msg::Ack(id().unwrap_or_default()),
        Some("error") => {
            return Msg::Error {
                id: id(),
                text: text.to_string(),
            };
        }
        Some("message") => {}
        _ => return Msg::Other,
    }
    let topic = v
        .get("topic")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let subject = v
        .get("subject")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let data = v
        .as_object_mut()
        .and_then(|m| m.remove("data"))
        .unwrap_or(Value::Null);
    if topic.starts_with("/spotMarket/level2Depth") {
        let symbol = topic.split(':').nth(1).unwrap_or_default().to_string();
        return depth(symbol, &data);
    }
    Msg::Data {
        topic,
        subject,
        data,
    }
}

/// Снимок стакана из data. Ошибок нет: неразборчивый — Msg::Bad.
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
    let ts = d.get("timestamp").and_then(Value::as_i64);
    Msg::Depth {
        symbol,
        bids,
        asks,
        exchange_time: ts.and_then(|ms| Utc.timestamp_millis_opt(ms).single()),
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

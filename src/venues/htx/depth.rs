//! Файл: venues/htx/depth.rs
//! Что делает: распаковка (GZIP) и разбор кадров рыночного WebSocket HTX: ping сервера, ответ на подписку, ошибка и
//! снимок стакана канала market.$symbol.mbp.refresh.$levels → стакан формата ядра. Один разбор JSON на кадр (урок 13).
//! Кто вызывает: ws_conn.rs, тесты.
//! От чего зависит: core/types, num.rs, flate2, serde_json.
//! Настройки: нет.
//!
//! Официально (https://huobiapi.github.io/docs/spot/v1/en/, «Websocket Market Data»):
//! - «All returned data of websocket Market APIs are compressed with GZIP»; сервер шлёт {"ping": <ms>} примерно раз в
//!   5 с, клиент отвечает {"pong": <то же число>}; после двух пропущенных ответов сервер рвёт соединение.
//! - Подписка {"sub": "<тема>", "id": "<свой id>"} → {"id", "status": "ok", "subbed", "ts"}; ошибка — {"status":
//!   "error", "err-code", "err-msg", "id", "ts"} (например bad-request).
//! - market.$symbol.mbp.refresh.$levels (5, 10, 20; «around 100ms») — снимок {"ch", "ts", "tick": {"seqNum", "bids",
//!   "asks"}}, уровни [цена, количество] числами JSON. Порядок уровней не оговорён — сортируем сами: asks по
//!   возрастанию цены, bids по убыванию.

use std::io::Read as _;

use chrono::{DateTime, TimeZone, Utc};
use rust_decimal::Decimal;
use serde_json::Value;

use crate::core::types::PriceLevel;

use super::num;

/// Что пришло в кадре.
pub enum Msg {
    /// Ping сервера: число, которое надо вернуть в pong.
    Ping(Value),
    /// Подписка принята (тема subbed).
    Subscribed(String),
    /// Ошибка сервера: id запроса (если есть) и текст как пришёл.
    Error {
        id: Option<String>,
        text: String,
    },
    /// Снимок стакана пары: символ HTX, seqNum, уровни, время биржи.
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

/// Распаковывает GZIP. Ошибка — текст причины.
pub fn gunzip(data: &[u8]) -> Result<String, String> {
    let mut s = String::new();
    flate2::read::GzDecoder::new(data)
        .read_to_string(&mut s)
        .map_err(|e| format!("GZIP: {e}"))?;
    Ok(s)
}

/// Символ из темы «market.<символ>.…». Ошибок нет.
pub fn symbol_of(ch: &str) -> Option<&str> {
    ch.strip_prefix("market.")?.split('.').next()
}

/// Разбирает текст кадра. Ошибок нет: неразборчивый — Msg::Bad или Other.
pub fn parse(text: &str) -> Msg {
    let Ok(v) = serde_json::from_str::<Value>(text) else {
        return Msg::Bad {
            symbol: None,
            reason: "кадр не JSON".into(),
        };
    };
    if let Some(p) = v.get("ping") {
        return Msg::Ping(p.clone());
    }
    if v.get("status").and_then(Value::as_str) == Some("error") {
        return Msg::Error {
            id: num::text_opt(&v, "id"),
            text: text.to_string(),
        };
    }
    if let Some(s) = v.get("subbed").and_then(Value::as_str) {
        return Msg::Subscribed(s.to_string());
    }
    let Some(ch) = v.get("ch").and_then(Value::as_str) else {
        return Msg::Other;
    };
    if !ch.contains(".mbp.refresh.") {
        return Msg::Other;
    }
    let symbol = symbol_of(ch).unwrap_or_default().to_string();
    let Some(tick) = v.get("tick") else {
        return Msg::Bad {
            symbol: Some(symbol),
            reason: "нет tick".into(),
        };
    };
    let (mut asks, mut bids) = match (levels(tick, "asks"), levels(tick, "bids")) {
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
    Msg::Depth {
        symbol,
        seq: tick.get("seqNum").and_then(Value::as_i64),
        bids,
        asks,
        exchange_time: v
            .get("ts")
            .and_then(Value::as_i64)
            .and_then(|ms| Utc.timestamp_millis_opt(ms).single()),
    }
}

/// Уровни [цена, количество] → PriceLevel; нулевое количество — уровня нет. Нет поля — пусто (сторона без заявок).
/// Ошибка — текст причины.
pub fn levels(tick: &Value, side: &str) -> Result<Vec<PriceLevel>, String> {
    let Some(arr) = tick.get(side) else {
        return Ok(vec![]);
    };
    let arr = arr.as_array().ok_or_else(|| format!("{side} не массив"))?;
    let mut out = Vec::with_capacity(arr.len());
    for l in arr {
        let p = l
            .as_array()
            .filter(|p| p.len() >= 2)
            .ok_or_else(|| format!("{side}: уровень не [цена, количество]"))?;
        let dec = |x: &Value, what: &str| {
            num::to_dec(x).ok_or_else(|| format!("{side}: {what} {x} не число"))
        };
        let (price, qty) = (dec(&p[0], "цена")?, dec(&p[1], "количество")?);
        if qty > Decimal::ZERO {
            out.push(PriceLevel { price, qty });
        }
    }
    Ok(out)
}

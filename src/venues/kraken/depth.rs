//! Файл: venues/kraken/depth.rs
//! Что делает: разбор текстовых кадров WebSocket v2 Kraken: pong, ответы на запросы (method subscribe / unsubscribe с
//! success / error), heartbeat и status, данные канала book (снимок и изменения по символам) и balances. Один разбор
//! JSON на кадр (урок 13).
//! Кто вызывает: ws_conn.rs, user_stream_conn.rs, тесты.
//! От чего зависит: core/types, num.rs, serde_json.
//! Настройки: нет.
//!
//! Официально (https://docs.kraken.com/exchange/api-reference/spot-websocket-v2/book.md): ответ на подписку — по
//! символу: {"method":"subscribe","result":{"channel","depth","snapshot","symbol"},"success":true|false,"error"};
//! данные — {"channel":"book","type":"snapshot"|"update","data":[{"symbol","bids":[{"price","qty"}],"asks":[…],
//! "checksum","timestamp"}]}; price / qty — «float» (числа JSON); «it is possible to have multiple updates to the same
//! price level in a single update message», «Updates should always be processed in sequence». Номера кадра нет —
//! целостность по checksum (local_book.rs). Канал heartbeat и pong — «соединение живо».

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde_json::Value;

use crate::core::types::PriceLevel;

use super::num;

/// Кадр стакана одного символа.
#[derive(Debug, Clone)]
pub struct Depth {
    pub symbol: String,
    /// true — снимок (type snapshot).
    pub snapshot: bool,
    /// Уровни как пришли (порядок сохранён; у изменения количество 0 — «убрать уровень»).
    pub bids: Vec<PriceLevel>,
    pub asks: Vec<PriceLevel>,
    pub checksum: Option<u32>,
    pub exchange_time: Option<DateTime<Utc>>,
}

/// Что пришло в кадре.
pub enum Msg {
    Pong,
    /// Ответ на запрос: method, success, символ из result (если есть), error, текст как пришёл.
    Ack {
        method: String,
        success: bool,
        symbol: Option<String>,
        error: Option<String>,
        text: String,
    },
    /// heartbeat / status — соединение живо.
    Alive,
    /// Канал book: кадры по символам; Err — символ (если известен) и причина неразбора.
    Book(Vec<Result<Depth, (Option<String>, String)>>),
    /// Канал balances: type (snapshot / update) и data.
    Balances {
        kind: String,
        data: Value,
    },
    /// Другой канал.
    Other(String),
    /// Кадр не разобран (причина).
    Bad(String),
}

/// Разбирает текстовый кадр. Ошибок нет: неразборчивый — Msg::Bad.
pub fn parse(text: &str) -> Msg {
    let Ok(mut v) = serde_json::from_str::<Value>(text) else {
        return Msg::Bad("кадр не JSON".into());
    };
    let s = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
    if let Some(method) = s(&v, "method") {
        if method == "pong" {
            return Msg::Pong;
        }
        return Msg::Ack {
            success: v.get("success").and_then(Value::as_bool).unwrap_or(false),
            symbol: v.get("result").and_then(|r| s(r, "symbol")),
            error: s(&v, "error"),
            method,
            text: text.to_string(),
        };
    }
    let channel = s(&v, "channel").unwrap_or_default();
    let kind = s(&v, "type").unwrap_or_default();
    let data = v
        .as_object_mut()
        .and_then(|m| m.remove("data"))
        .unwrap_or(Value::Null);
    match channel.as_str() {
        "heartbeat" | "status" => Msg::Alive,
        "book" => {
            let snapshot = kind == "snapshot";
            let rows = data.as_array().map(Vec::as_slice).unwrap_or_default();
            Msg::Book(rows.iter().map(|d| depth(d, snapshot)).collect())
        }
        "balances" => Msg::Balances { kind, data },
        other => Msg::Other(other.to_string()),
    }
}

/// Кадр стакана одного символа из строки data. Ошибка — (символ, причина).
fn depth(d: &Value, snapshot: bool) -> Result<Depth, (Option<String>, String)> {
    let symbol = num::text_opt(d, "symbol");
    let fail = |r: String| (symbol.clone(), r);
    let sym = symbol.clone().ok_or_else(|| fail("нет symbol".into()))?;
    let bids = levels(d, "bids").map_err(fail)?;
    let asks = levels(d, "asks").map_err(fail)?;
    let checksum = d
        .get("checksum")
        .and_then(|c| c.as_u64().or_else(|| c.as_str()?.parse().ok()))
        .and_then(|c| u32::try_from(c).ok());
    let exchange_time = num::text_opt(d, "timestamp")
        .and_then(|t| DateTime::parse_from_rfc3339(&t).ok())
        .map(|t| t.with_timezone(&Utc));
    Ok(Depth {
        symbol: sym,
        snapshot,
        bids,
        asks,
        checksum,
        exchange_time,
    })
}

/// Уровни [{price, qty}] → PriceLevel (количество 0 сохраняется — у изменения «убрать уровень»). Ошибка — причина.
pub fn levels(d: &Value, side: &str) -> Result<Vec<PriceLevel>, String> {
    let Some(arr) = d.get(side).and_then(Value::as_array) else {
        return Ok(vec![]);
    };
    let mut out = Vec::with_capacity(arr.len());
    for l in arr {
        let n = |k: &str| -> Result<Decimal, String> {
            l.get(k)
                .and_then(num::to_dec)
                .ok_or_else(|| format!("{side}: {k} не число ({l})"))
        };
        out.push(PriceLevel {
            price: n("price")?,
            qty: n("qty")?,
        });
    }
    Ok(out)
}

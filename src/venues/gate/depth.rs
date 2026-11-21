//! Файл: venues/gate/depth.rs
//! Что делает: разбор текстовых кадров WebSocket Gate (JSON): ответ на подписку, spot.pong, уведомление о
//! перезапуске (spot.system), снимок стакана канала spot.order_book → OrderBook формата ядра; запросы подписки и пинга.
//! Кто вызывает: ws_conn.rs, feed.rs, user_stream_conn.rs (ответы, пинг), tap.rs (разбор кадра окна).
//! От чего зависит: num.rs, core/types.
//! Настройки: нет.
//!
//! Официально (Spot WebSocket v4, https://www.gate.com/docs/developers/apiv4/ws/en/):
//! - запрос: {time (секунды), id, channel, event: subscribe | unsubscribe, payload}; ответ: {time, time_ms, id, channel,
//!   event, error, result}; подписка принята — error = null и result {"status":"success"}; коды error: 1 — неверный
//!   формат, 2 — неверный аргумент, 3 — ошибка сервера, 4 — не прошла аутентификация;
//! - пинг приложения: channel spot.ping → ответ channel spot.pong («will also reset the client's timeout timer»);
//! - spot.system с result.type = upgrade — сервер обновляется, переподключиться как можно скорее;
//! - spot.order_book: payload [пара, уровни, интервал], уровни 5, 10, 20, 50, 100, интервал 100ms или 1000ms;
//!   обновление (event update) — result {t, lastUpdateId, s, l, bids — «high to low», asks — «low to high»}, уровни
//!   [цена, количество] строками. Каждый кадр — снимок уровней (не изменения).
//!
//! Порядок уровней документирован, но сортируем сами (как BingX и MEXC) — цена не зависит от порядка в кадре.

use std::str::FromStr;

use chrono::{DateTime, TimeZone, Utc};
use rust_decimal::Decimal;
use serde_json::{Value, json};

use crate::core::types::{Asset, OrderBook, PriceLevel};

/// Допустимые уровни spot.order_book — из официальной документации.
pub const DEPTH_LEVELS: [u64; 5] = [5, 10, 20, 50, 100];
/// Допустимые интервалы spot.order_book — из официальной документации.
pub const BOOK_INTERVALS: [&str; 2] = ["100ms", "1000ms"];
/// Канал снимков стакана (официальное имя).
pub const BOOK_CHANNEL: &str = "spot.order_book";

/// Что пришло в кадре.
pub enum Msg {
    /// Ответ spot.pong на наш spot.ping.
    Pong,
    /// Снимок стакана пары (символ Gate; монету подставляет вызывающий).
    Depth {
        symbol: String,
        update_id: i64,
        bids: Vec<PriceLevel>,
        asks: Vec<PriceLevel>,
        exchange_time: Option<DateTime<Utc>>,
    },
    /// Ответ на подписку или отписку: id запроса, ошибка (None — принято).
    Ack {
        id: Option<i64>,
        error: Option<String>,
    },
    /// Сервер обновляется (spot.system, upgrade) — переподключиться.
    Upgrade,
    /// Кадр стакана не разобран — символ, если известен, и причина.
    Bad {
        symbol: Option<String>,
        reason: String,
    },
    /// Другое (только в сырых сообщениях).
    Other,
}

/// Запрос подписки или отписки. Принимает: канал, событие, payload, id. Возвращает: текст JSON. Ошибок нет.
pub fn request(channel: &str, event: &str, payload: Value, id: i64) -> String {
    let t = Utc::now().timestamp();
    json!({ "time": t, "id": id, "channel": channel, "event": event, "payload": payload })
        .to_string()
}

/// Пинг приложения: {"time": …, "channel": "spot.ping"}. Ошибок нет.
pub fn ping() -> String {
    json!({ "time": Utc::now().timestamp(), "channel": "spot.ping" }).to_string()
}

/// Разбирает текстовый кадр. Ошибок нет: неразборчивый стакан — Msg::Bad, прочее — Msg::Other.
pub fn parse(text: &str) -> Msg {
    let Ok(v) = serde_json::from_str::<Value>(text) else {
        return Msg::Other;
    };
    let channel = v.get("channel").and_then(Value::as_str).unwrap_or_default();
    let event = v.get("event").and_then(Value::as_str).unwrap_or_default();
    if channel == "spot.pong" {
        return Msg::Pong;
    }
    if channel == "spot.system"
        && v.pointer("/result/type").and_then(Value::as_str) == Some("upgrade")
    {
        return Msg::Upgrade;
    }
    if matches!(event, "subscribe" | "unsubscribe") {
        let error = v
            .get("error")
            .filter(|e| !e.is_null())
            .map(Value::to_string);
        return Msg::Ack {
            id: v.get("id").and_then(Value::as_i64),
            error,
        };
    }
    if channel == BOOK_CHANNEL && event == "update" {
        return book(v.get("result").unwrap_or(&Value::Null));
    }
    Msg::Other
}

/// Снимок стакана из result. Ошибок нет: неразборчивый — Msg::Bad.
fn book(r: &Value) -> Msg {
    let symbol = r.get("s").and_then(Value::as_str).map(str::to_string);
    let Some(sym) = symbol.clone() else {
        return Msg::Bad {
            symbol: None,
            reason: "стакан без поля s".into(),
        };
    };
    let Some(update_id) = r.get("lastUpdateId").and_then(Value::as_i64) else {
        return Msg::Bad {
            symbol,
            reason: "стакан без lastUpdateId".into(),
        };
    };
    let (mut asks, mut bids) = match (levels(r, "asks"), levels(r, "bids")) {
        (Ok(a), Ok(b)) => (a, b),
        (Err(reason), _) | (_, Err(reason)) => return Msg::Bad { symbol, reason },
    };
    asks.sort_by_key(|a| a.price);
    bids.sort_by_key(|b| std::cmp::Reverse(b.price));
    let exchange_time = r
        .get("t")
        .and_then(Value::as_i64)
        .and_then(|ms| Utc.timestamp_millis_opt(ms).single());
    Msg::Depth {
        symbol: sym,
        update_id,
        bids,
        asks,
        exchange_time,
    }
}

/// Уровни [цена, количество] (строки → Decimal). Нулевое количество — уровня нет. Ошибка — текст причины.
pub fn levels(r: &Value, side: &str) -> Result<Vec<PriceLevel>, String> {
    let arr = r
        .get(side)
        .and_then(Value::as_array)
        .ok_or(format!("нет {side}"))?;
    let mut out = Vec::with_capacity(arr.len());
    for l in arr {
        let pair = l
            .as_array()
            .filter(|p| p.len() >= 2)
            .ok_or(format!("{side}: уровень не [цена, количество]"))?;
        let num = |x: &Value| x.as_str().and_then(|s| Decimal::from_str(s.trim()).ok());
        let price = num(&pair[0]).ok_or(format!("{side}: цена не число"))?;
        let qty = num(&pair[1]).ok_or(format!("{side}: количество не число"))?;
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

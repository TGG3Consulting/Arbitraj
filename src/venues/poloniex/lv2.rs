//! Файл: venues/poloniex/lv2.rs
//! Что делает: решение 424 — разбор кадров канала book_lv2 Poloniex (снимок полного стакана 20 уровней, затем изменения)
//! в записи для локального стакана (local_book.rs). Количество 0 в изменении сохраняется — это команда «удалить цену».
//! Кто вызывает: depth.rs (parse — по каналу кадра), ws_conn.rs (сообщения подписки), тесты.
//! От чего зависит: depth.rs (Msg, levels_with), core/types, serde_json.
//! Настройки: нет.
//!
//! Официально (Market Data → Book Level 2, https://api-docs.poloniex.com/spot/websocket/market-data):
//! - подписка {"event": "subscribe", "channel": ["book_lv2"], "symbols": [...]}; параметра depth у book_lv2 нет —
//!   «Receive a snapshot of the full 20 level order book», затем изменения;
//! - кадр {"channel": "book_lv2", "action": "snapshot" | "update", "data": [{symbol, asks, bids, createTime, lastId, id,
//!   ts}]}; уровни [цена, количество] строками; lastId — id предыдущего сообщения, id — «SeqId»; createTime и ts —
//!   числа (пример офдоки: snapshot lastId 10409, id 10410; update lastId 10410, id 10421 — id растёт не на 1, цепочку
//!   задаёт lastId);
//! - частота book_lv2 в документации не указана (у book — «updated every 100ms») — замерить на живой записи (TASKS).

use chrono::{DateTime, TimeZone, Utc};
use serde_json::{Value, json};

use crate::core::types::PriceLevel;

use super::depth::{Msg, levels_with};

/// Канал стакана изменениями (официальное имя).
pub const BOOK_LV2: &str = "book_lv2";

/// Одна запись data кадра book_lv2.
#[derive(Debug, Clone)]
pub struct Lv2 {
    pub symbol: String,
    /// action = snapshot (иначе update).
    pub snapshot: bool,
    /// id записи (SeqId).
    pub id: Option<i64>,
    /// lastId — id предыдущего сообщения.
    pub last_id: Option<i64>,
    /// Уровни в порядке кадра; количество 0 — удалить цену.
    pub bids: Vec<PriceLevel>,
    pub asks: Vec<PriceLevel>,
    /// Время создания записи биржей (createTime).
    pub exchange_time: Option<DateTime<Utc>>,
}

/// Что делает: сообщение подписки на book_lv2 для одного символа (без depth). Ошибок нет.
pub fn sub_message(symbol: &str) -> String {
    json!({"event": "subscribe", "channel": [BOOK_LV2], "symbols": [symbol]}).to_string()
}

/// Что делает: сообщение отписки book_lv2 одного символа (Spot → WebSocket, «Unsubscribe» — тот же channel и
/// symbols). Ошибок нет.
pub fn unsub_message(symbol: &str) -> String {
    json!({"event": "unsubscribe", "channel": [BOOK_LV2], "symbols": [symbol]}).to_string()
}

/// Что делает: разбирает кадр канала book_lv2 (JSON уже разобран depth.rs). Принимает: кадр. Возвращает: Msg::Lv2
/// или Msg::BadLv2 (символ, если известен, и причина). Ошибок нет.
pub fn parse(v: &Value) -> Msg {
    let snapshot = match v.get("action").and_then(Value::as_str) {
        Some("snapshot") => true,
        Some("update") => false,
        other => {
            return Msg::BadLv2 {
                symbol: first_symbol(v),
                reason: format!("book_lv2: action {other:?} — ждали snapshot или update"),
            };
        }
    };
    let Some(items) = v.get("data").and_then(Value::as_array) else {
        return Msg::BadLv2 {
            symbol: None,
            reason: "book_lv2: нет массива data".into(),
        };
    };
    let mut out = Vec::with_capacity(items.len());
    for d in items {
        let symbol = d.get("symbol").and_then(Value::as_str).unwrap_or_default();
        match record(symbol, snapshot, d) {
            Ok(x) => out.push(x),
            Err(reason) => {
                return Msg::BadLv2 {
                    symbol: Some(symbol.to_string()).filter(|s| !s.is_empty()),
                    reason: format!("book_lv2: {reason}"),
                };
            }
        }
    }
    Msg::Lv2(out)
}

/// Символ первой записи (для неразборчивого кадра). Ошибок нет.
fn first_symbol(v: &Value) -> Option<String> {
    v.get("data")?
        .as_array()?
        .first()?
        .get("symbol")?
        .as_str()
        .map(str::to_string)
}

/// Одна запись data. Ошибка — текст причины.
fn record(symbol: &str, snapshot: bool, d: &Value) -> Result<Lv2, String> {
    if symbol.is_empty() {
        return Err("запись без symbol".into());
    }
    let int = |f: &str| match d.get(f) {
        Some(Value::String(s)) => s.parse::<i64>().ok(),
        Some(x) => x.as_i64(),
        None => None,
    };
    Ok(Lv2 {
        symbol: symbol.to_string(),
        snapshot,
        id: int("id"),
        last_id: int("lastId"),
        bids: levels_with(d, "bids", !snapshot)?,
        asks: levels_with(d, "asks", !snapshot)?,
        exchange_time: int("createTime").and_then(|ms| Utc.timestamp_millis_opt(ms).single()),
    })
}

//! Файл: venues/coinex/ws_msg.rs
//! Что делает: тексты запросов WebSocket стаканов CoinEx — depth.subscribe (при подключении и при переподписке сторожем
//! пар, решение 202) и depth.unsubscribe всех рынков соединения.
//! Кто вызывает: ws_conn.rs, тесты.
//! От чего зависит: serde_json.
//! Настройки: нет (рынки и limit передаёт вызывающий).
//!
//! Официально (WS Market Depth, https://docs.coinex.com/api/v2/spot/market/ws/market-depth): depth.subscribe —
//! market_list [[market, limit, interval, if_full], …] (пример ["BTCUSDT", 10, "0", true]); depth.unsubscribe —
//! market_list рынков, пример «All markets»: {"method": "depth.unsubscribe", "params": {"market_list": []}, "id": 1}.

use serde_json::{Value, json};

/// id запроса подписки (ping идут со следующих номеров).
pub const SUB_ID: i64 = 1;

/// Что делает: запрос подписки на стаканы рынков. Принимает: рынки, limit. Возвращает: текст запроса. Ошибок нет.
pub fn sub_message(symbols: &[&str], levels: u64) -> String {
    sub_message_id(symbols, levels, SUB_ID)
}

/// Что делает: запрос подписки с заданным id (решение 202: переподписка сторожем — свой id для разбора отказа).
/// Принимает: рынки, limit, id. Возвращает: текст запроса. Ошибок нет.
pub fn sub_message_id(symbols: &[&str], levels: u64, id: i64) -> String {
    let list: Vec<Value> = symbols
        .iter()
        .map(|s| json!([s, levels, "0", true]))
        .collect();
    json!({"method": "depth.subscribe", "params": {"market_list": list}, "id": id}).to_string()
}

/// Что делает: запрос отписки от стаканов всех рынков соединения (market_list пуст — «unsubscribe to all markets»).
/// Принимает: id. Возвращает: текст запроса. Ошибок нет.
pub fn unsub_all_message(id: i64) -> String {
    json!({"method": "depth.unsubscribe", "params": {"market_list": []}, "id": id}).to_string()
}

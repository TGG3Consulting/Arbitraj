//! Файл: venues/coinex/depth.rs
//! Что делает: разбор текстовых кадров WebSocket CoinEx (после распаковки, inflate.rs): ответы на запросы (подписка,
//! server.ping → "pong", ошибки по id) и пуш depth.update → стакан формата ядра. Один разбор JSON на кадр (урок 13).
//! Кто вызывает: ws_conn.rs, тесты.
//! От чего зависит: core/types, serde_json.
//! Настройки: нет.
//!
//! Официально:
//! - Integration Guide (https://docs.coinex.com/api/v2/guide): ответ на запрос — {id, code, message}, code 0 — успех.
//! - Ping (https://docs.coinex.com/api/v2/common/ws/ping): ответ {"id":1,"code":0,"data":{"result":"pong"},…}.
//! - WS Market Depth (https://docs.coinex.com/api/v2/spot/market/ws/market-depth): пуш {"method":"depth.update","data":
//!   {"market", "is_full", "depth": {"asks": [[цена, количество]], "bids": […], "last", "updated_at", "checksum"}},
//!   "id": null}; is_full — «true for a full push, false for an incremental push»; количество "0" в инкременте — удалить
//!   уровень. Порядок уровней не оговорён — сортируем сами: asks по возрастанию цены, bids по убыванию.
//!   checksum (CRC32 строки уровней) не сверяется: покрывает ли он N присланных уровней или весь стакан, в тексте не
//!   однозначно («covers the full depth data»), а в примере число 2578768879 не помещается в signed int32 (урок 2) —
//!   проверить на живой записи.
//! - Ошибки WS (https://docs.coinex.com/api/v2/error): 20001 параметры, 20002 нет метода, 21001 нужна подпись, 21002
//!   подпись не прошла, 23001 таймаут, 23002 «Requests submitted too frequently», 24001, 24002.

use std::str::FromStr;

use rust_decimal::Decimal;
use serde_json::Value;

use crate::core::types::PriceLevel;

/// Что пришло в кадре.
pub enum Msg {
    /// Ответ на server.ping.
    Pong,
    /// Ответ code 0 на запрос с этим id (подписка принята).
    Ack(i64),
    /// Ответ с ошибкой: id запроса, код и кадр как пришёл.
    Error {
        id: Option<i64>,
        code: i64,
        text: String,
    },
    /// Стакан пары: рынок, полный ли пуш, уровни, время биржи (мс).
    Depth {
        symbol: String,
        full: bool,
        bids: Vec<PriceLevel>,
        asks: Vec<PriceLevel>,
        updated_at: Option<i64>,
    },
    /// Кадр не разобран (рынок, если известен, и причина).
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
    if v.get("method").and_then(Value::as_str) == Some("depth.update") {
        return depth(&v);
    }
    let Some(code) = v.get("code").and_then(Value::as_i64) else {
        return Msg::Other;
    };
    let id = v.get("id").and_then(Value::as_i64);
    if code != 0 {
        return Msg::Error {
            id,
            code,
            text: text.to_string(),
        };
    }
    let pong = v
        .get("data")
        .and_then(|d| d.get("result"))
        .and_then(Value::as_str)
        == Some("pong");
    match (pong, id) {
        (true, _) => Msg::Pong,
        (false, Some(id)) => Msg::Ack(id),
        (false, None) => Msg::Other,
    }
}

/// Пуш depth.update. Ошибок нет: неразборчивый — Msg::Bad.
fn depth(v: &Value) -> Msg {
    let data = v.get("data");
    let symbol = data
        .and_then(|d| d.get("market"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let Some((symbol, d)) = symbol.zip(data) else {
        return Msg::Bad {
            symbol: None,
            reason: "depth.update без data.market".into(),
        };
    };
    let Some(book) = d.get("depth") else {
        return Msg::Bad {
            symbol: Some(symbol),
            reason: "нет data.depth".into(),
        };
    };
    let (mut asks, mut bids) = match (levels(book, "asks"), levels(book, "bids")) {
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
        full: d.get("is_full").and_then(Value::as_bool) == Some(true),
        bids,
        asks,
        updated_at: book.get("updated_at").and_then(Value::as_i64),
    }
}

/// Уровни [цена, количество] (строки) → PriceLevel; нулевое количество — уровня нет. Нет стороны — пусто (у стакана
/// без заявок одной стороны). Ошибка — текст причины.
pub fn levels(d: &Value, side: &str) -> Result<Vec<PriceLevel>, String> {
    let Some(arr) = d.get(side) else {
        return Ok(Vec::new());
    };
    let arr = arr.as_array().ok_or_else(|| format!("{side} не массив"))?;
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

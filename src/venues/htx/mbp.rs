//! Файл: venues/htx/mbp.rs
//! Что делает: решение 422 — разбор кадров быстрого канала стакана HTX (адрес /feed): изменения
//! market.$symbol.mbp.$levels и ответ на запрос снимка {"req": тема} того же канала; сообщения подписки и запроса.
//! Ping, ответ на подписку и ошибка — как у кадров /ws (depth.rs). Один разбор JSON на кадр (урок 13).
//! Кто вызывает: fast_conn.rs, fast_pair.rs, тесты.
//! От чего зависит: core/types, depth.rs (Msg — общие виды служебных кадров), num.rs, serde_json.
//! Настройки: нет.
//!
//! Официально (https://huobiapi.github.io/docs/spot/v1/en/, «Websocket Market Data → Market By Price (incremental
//! update)»): адрес «wss://api.huobi.pro/feed» или «wss://api-aws.huobi.pro/feed» (настройка venue.ws_feed_url); тема
//! market.$symbol.mbp.$levels, уровни 5, 20, 150, 400 (запрос снимка — только 5, 20, 150); подписка
//! {"sub": "market.btcusdt.mbp.5", "id": "id1"}; изменение {"ch", "ts", "tick": {"seqNum", "prevSeqNum", "asks"
//! или "bids" — [price, size]}} (5/20 — только изменившаяся сторона); запрос снимка {"req": "market.btcusdt.mbp.5",
//! "id": "id2"} → {"id", "rep", "status": "ok", "data": {"seqNum", "bids", "asks"}} (ts в примере ответа нет);
//! «size» 0 — уровень удалить (шаг 7 процедуры).

use serde_json::{Value, json};

use crate::core::types::PriceLevel;

use super::depth::{self, Msg};
use super::num;

/// Префикс своего id запроса снимка (id подписки — символ, как у /ws; по префиксу видно, что ошибка — у запроса).
pub const REQ_ID: &str = "req.";

/// Что пришло в кадре /feed.
pub enum Feed {
    /// Служебный кадр (ping, подписка принята, ошибка) — как у /ws.
    Service(Msg),
    /// Изменение стакана пары: символ, seqNum, prevSeqNum, уровни (количество 0 — убрать), время биржи (мс).
    Update {
        symbol: String,
        seq: i64,
        prev: i64,
        bids: Vec<PriceLevel>,
        asks: Vec<PriceLevel>,
        ts: Option<i64>,
    },
    /// Снимок по запросу: символ, seqNum, уровни.
    Refresh {
        symbol: String,
        seq: i64,
        bids: Vec<PriceLevel>,
        asks: Vec<PriceLevel>,
    },
    /// Кадр стакана не разобран (символ, если известен, и причина).
    Bad {
        symbol: Option<String>,
        reason: String,
    },
    Other,
}

/// Тема канала. Ошибок нет.
pub fn topic(symbol: &str, levels: u64) -> String {
    format!("market.{symbol}.mbp.{levels}")
}

/// Подписка на изменения символа (id — символ). Ошибок нет.
pub fn sub_message(symbol: &str, levels: u64) -> String {
    json!({"sub": topic(symbol, levels), "id": symbol}).to_string()
}

/// Запрос снимка символа тем же каналом (id — REQ_ID + символ). Ошибок нет.
pub fn req_message(symbol: &str, levels: u64) -> String {
    json!({"req": topic(symbol, levels), "id": format!("{REQ_ID}{symbol}")}).to_string()
}

/// Разбирает текст кадра /feed. Ошибок нет: неразборчивый — Feed::Bad или Other.
pub fn parse(text: &str) -> Feed {
    let Ok(v) = serde_json::from_str::<Value>(text) else {
        return Feed::Bad {
            symbol: None,
            reason: "кадр не JSON".into(),
        };
    };
    if v.get("ping").is_some()
        || v.get("subbed").is_some()
        || v.get("status").and_then(Value::as_str) == Some("error")
    {
        // Служебные виды — тем же разбором, что у /ws (тот же текст — один повторный разбор на редкий кадр).
        return Feed::Service(depth::parse(text));
    }
    if let Some(rep) = v.get("rep").and_then(Value::as_str) {
        return refresh(rep, &v);
    }
    let Some(ch) = v.get("ch").and_then(Value::as_str) else {
        return Feed::Other;
    };
    if !ch.contains(".mbp.") || ch.contains(".mbp.refresh.") {
        return Feed::Other;
    }
    let symbol = depth::symbol_of(ch).unwrap_or_default().to_string();
    let bad = |reason: String| Feed::Bad {
        symbol: Some(symbol.clone()),
        reason,
    };
    let Some(tick) = v.get("tick") else {
        return bad("нет tick".into());
    };
    let (Some(seq), Some(prev)) = (
        tick.get("seqNum").and_then(Value::as_i64),
        tick.get("prevSeqNum").and_then(Value::as_i64),
    ) else {
        return bad("нет seqNum или prevSeqNum".into());
    };
    match (levels(tick, "bids"), levels(tick, "asks")) {
        (Ok(bids), Ok(asks)) => Feed::Update {
            symbol,
            seq,
            prev,
            bids,
            asks,
            ts: v.get("ts").and_then(Value::as_i64),
        },
        (Err(r), _) | (_, Err(r)) => bad(r),
    }
}

/// Ответ на запрос снимка: {"rep", "status": "ok", "data": {"seqNum", "bids", "asks"}}. Ошибок нет: другой вид —
/// Feed::Bad с символом темы.
fn refresh(rep: &str, v: &Value) -> Feed {
    let symbol = depth::symbol_of(rep).unwrap_or_default().to_string();
    let bad = |reason: &str| Feed::Bad {
        symbol: Some(symbol.clone()),
        reason: format!("ответ на запрос снимка: {reason}"),
    };
    if v.get("status").and_then(Value::as_str) != Some("ok") {
        return bad("status не ok");
    }
    let Some(data) = v.get("data") else {
        return bad("нет data");
    };
    let Some(seq) = data.get("seqNum").and_then(Value::as_i64) else {
        return bad("нет seqNum");
    };
    match (levels(data, "bids"), levels(data, "asks")) {
        (Ok(bids), Ok(asks)) => Feed::Refresh {
            symbol,
            seq,
            bids,
            asks,
        },
        (Err(r), _) | (_, Err(r)) => bad(&r),
    }
}

/// Уровни [price, size] стороны; количество 0 сохраняется (в изменении это «убрать уровень»). Нет поля — пусто
/// (у 5/20 приходит только изменившаяся сторона). Ошибка — текст причины.
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
        out.push(PriceLevel {
            price: dec(&p[0], "цена")?,
            qty: dec(&p[1], "количество")?,
        });
    }
    Ok(out)
}

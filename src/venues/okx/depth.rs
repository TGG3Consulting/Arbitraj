//! Файл: venues/okx/depth.rs
//! Что делает: разбор текстовых кадров WebSocket OKX: «pong», ответы на запросы (event subscribe / unsubscribe / login /
//! error / notice) и данные каналов — стакан books5 и books (уровни, ts, seqId, prevSeqId, action) или данные другого
//! канала (deposit-info и др.). Один разбор JSON на кадр (урок 13).
//! Кто вызывает: ws_conn.rs, user_stream_conn.rs, тесты.
//! От чего зависит: core/types, serde_json.
//! Настройки: нет.
//!
//! Официально (https://www.okx.com/docs-v5/en/):
//! - «Overview → WebSocket → Connect»: «send the String 'ping'», «expect a 'pong' as a response»;
//! - «Overview → WebSocket → Subscribe»: ответ {"event":"subscribe","arg":{…},"connId"}; отказ {"event":"error","code",
//!   "msg","connId"} (например 60012 «Invalid request»);
//! - «Order Book Trading → Market Data → WS / Order book channel»: данные {"arg":{"channel","instId"},"action":
//!   "snapshot"|"update","data":[{"asks","bids","ts","checksum","prevSeqId","seqId"}]}; уровень ["411.8","10","0","4"] —
//!   «depth price», «quantity at the price» (у спота — в базовой монете), устаревшее «0», число ордеров; у books5 поля
//!   action нет (каждый кадр — снимок 5 уровней). Порядок уровней в документации не оговорён — сортируем сами.

use std::str::FromStr;

use chrono::{DateTime, TimeZone, Utc};
use rust_decimal::Decimal;
use serde_json::Value;

use crate::core::types::PriceLevel;

/// Кадр стакана.
pub struct Depth {
    /// Канал: books5 или books.
    pub channel: String,
    /// instId пары.
    pub inst: String,
    /// true — снимок (books5 — всегда; books — action snapshot).
    pub snapshot: bool,
    /// Уровни как пришли (у изменения books количество 0 — «убрать уровень»).
    pub bids: Vec<PriceLevel>,
    pub asks: Vec<PriceLevel>,
    pub seq: Option<i64>,
    pub prev_seq: Option<i64>,
    pub exchange_time: Option<DateTime<Utc>>,
}

/// Что пришло в кадре.
pub enum Msg {
    Pong,
    /// Ответ на запрос: event (subscribe, unsubscribe, login, error, notice…), code, msg, instId из arg (если есть),
    /// текст как пришёл.
    Event {
        event: String,
        code: String,
        msg: String,
        inst: Option<String>,
        text: String,
    },
    Depth(Depth),
    /// Данные другого канала (канал и строки data).
    Data {
        channel: String,
        data: Value,
    },
    /// Кадр не разобран (instId, если известен, и причина).
    Bad {
        inst: Option<String>,
        reason: String,
    },
}

/// Разбирает текстовый кадр. Ошибок нет: неразборчивый — Msg::Bad.
pub fn parse(text: &str) -> Msg {
    if text == "pong" {
        return Msg::Pong;
    }
    let Ok(mut v) = serde_json::from_str::<Value>(text) else {
        return Msg::Bad {
            inst: None,
            reason: "кадр не JSON".into(),
        };
    };
    let s = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
    let arg = v.get("arg").cloned().unwrap_or(Value::Null);
    let inst = s(&arg, "instId");
    if let Some(event) = s(&v, "event") {
        return Msg::Event {
            event,
            code: s(&v, "code").unwrap_or_default(),
            msg: s(&v, "msg").unwrap_or_default(),
            inst,
            text: text.to_string(),
        };
    }
    let channel = s(&arg, "channel").unwrap_or_default();
    let action = s(&v, "action");
    let data = v
        .as_object_mut()
        .and_then(|m| m.remove("data"))
        .unwrap_or(Value::Null);
    if channel == "books5" || channel == "books" {
        return depth(channel, inst, action, &data);
    }
    Msg::Data { channel, data }
}

/// Кадр стакана из data. Ошибок нет: неразборчивый — Msg::Bad.
fn depth(channel: String, inst: Option<String>, action: Option<String>, data: &Value) -> Msg {
    let bad = |reason: String| Msg::Bad {
        inst: inst.clone(),
        reason,
    };
    let Some(inst_id) = inst.clone() else {
        return bad("нет arg.instId".into());
    };
    let Some(d) = data.as_array().and_then(|a| a.first()) else {
        return bad("data пустой".into());
    };
    let (mut asks, mut bids) = match (levels(d, "asks"), levels(d, "bids")) {
        (Ok(a), Ok(b)) => (a, b),
        (Err(r), _) | (_, Err(r)) => return bad(r),
    };
    asks.sort_by_key(|a| a.price);
    bids.sort_by_key(|b| std::cmp::Reverse(b.price));
    let int = |k: &str| {
        d.get(k)
            .and_then(|x| x.as_i64().or_else(|| x.as_str()?.parse().ok()))
    };
    let snapshot = channel == "books5" || action.as_deref() == Some("snapshot");
    Msg::Depth(Depth {
        channel,
        inst: inst_id,
        snapshot,
        bids,
        asks,
        seq: int("seqId"),
        prev_seq: int("prevSeqId"),
        exchange_time: int("ts").and_then(|ms| Utc.timestamp_millis_opt(ms).single()),
    })
}

/// Уровни [цена, количество, …] (строки) → PriceLevel, количество 0 сохраняется (у изменения — «убрать уровень»).
/// Ошибка — текст причины.
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
            .ok_or_else(|| format!("{side}: уровень не [цена, количество, …]"))?;
        let num = |x: &Value, what: &str| {
            x.as_str()
                .and_then(|s| Decimal::from_str(s.trim()).ok())
                .ok_or_else(|| format!("{side}: {what} {x} не число"))
        };
        out.push(PriceLevel {
            price: num(&p[0], "цена")?,
            qty: num(&p[1], "количество")?,
        });
    }
    Ok(out)
}

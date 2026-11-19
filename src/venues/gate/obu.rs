//! Файл: venues/gate/obu.rs
//! Что делает: решение 421 — разбор кадров канала spot.obu (Order Book V2) Gate: полный снимок (full = true) и
//! изменения (U/u), ответ на подписку и отписку; имя потока «ob.<пара>.<уровни>» и запрос подписки.
//! Кто вызывает: ws_conn.rs (каждый текстовый кадр — сначала сюда), fast_book.rs (Frame), тесты.
//! От чего зависит: depth.rs (request — тот же формат запроса Spot WebSocket v4), core/types (PriceLevel).
//! Настройки: нет (уровни потока — venue.obu_level, читает ws.rs).
//!
//! Официально (Spot WebSocket v4, раздел «Order Book V2 API», канал spot.obu,
//! https://www.gate.com/docs/developers/apiv4/ws/en/; журнал изменений 2025-06-24):
//! - payload подписки — ["ob.{symbol}.{level}"], например «ob.BTC_USDT.50»; отписка — event unsubscribe с тем же payload;
//! - «The level enum values are: 400, 50; Update frequency: 400-level updates every 100 ms; 50-level updates every
//!   20 ms»;
//! - result: t (Integer) — время построения стакана, мс; full (Boolean) — «true indicates a full depth snapshot; false
//!   indicates incremental depth updates» (при false поля нет); s — имя потока; U — «Starting order book update ID of
//!   this update»; u — «Ending order book update ID of this update»; b / a — [цена, количество] строками, количество
//!   «0» — убрать уровень. В примере полного снимка поля U нет (только u);
//! - «For the same spot trading pair and the same depth stream, a single connection is allowed to subscribe only once».
//! В примере уведомления поля event нет — уведомлением считается кадр spot.obu с result и без event subscribe /
//! unsubscribe.

use std::str::FromStr;

use rust_decimal::Decimal;
use serde_json::{Value, json};

use crate::core::types::PriceLevel;

use super::depth;

/// Канал Order Book V2 (официальное имя).
pub const OBU_CHANNEL: &str = "spot.obu";
/// Допустимые уровни потока — из официальной документации («The level enum values are: 400, 50»).
pub const OBU_LEVELS: [u64; 2] = [50, 400];

/// Кадр стакана spot.obu.
#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    /// Имя потока «ob.<пара>.<уровни>» (поле s).
    pub stream: String,
    /// Полный снимок (full = true).
    pub full: bool,
    /// U — первый номер изменения кадра (у полного снимка в примере документации нет).
    pub first: Option<i64>,
    /// u — последний номер изменения кадра.
    pub last: i64,
    /// t — время построения стакана, мс (Gate).
    pub t: Option<i64>,
    /// Изменения bids / asks: количество 0 — убрать уровень.
    pub bids: Vec<PriceLevel>,
    pub asks: Vec<PriceLevel>,
}

/// Что пришло в кадре spot.obu.
#[derive(Debug, Clone, PartialEq)]
pub enum Obu {
    /// Снимок или изменения стакана.
    Update(Frame),
    /// Ответ на подписку или отписку: событие, поток из payload, ошибка (None — принято).
    Ack {
        event: String,
        stream: Option<String>,
        error: Option<String>,
    },
    /// Кадр не разобран: поток, если известен, и причина.
    Bad {
        stream: Option<String>,
        reason: String,
    },
}

/// Имя потока «ob.<пара>.<уровни>» (payload подписки). Ошибок нет.
pub fn stream(symbol: &str, level: u64) -> String {
    format!("ob.{symbol}.{level}")
}

/// Символ пары из имени потока: «ob.CAKE_USDT.50» → «CAKE_USDT». Возвращает: None — имя не того вида. Ошибок нет.
pub fn symbol_of(stream: &str) -> Option<&str> {
    let rest = stream.strip_prefix("ob.")?;
    let (sym, level) = rest.rsplit_once('.')?;
    level.parse::<u64>().ok().map(|_| sym)
}

/// Запрос подписки или отписки потока. Принимает: событие (subscribe | unsubscribe), поток, id. Возвращает: текст JSON.
pub fn request(event: &str, stream: &str, id: i64) -> String {
    depth::request(OBU_CHANNEL, event, json!([stream]), id)
}

/// Разбирает текстовый кадр. Возвращает: None — кадр не канала spot.obu (его разбирает depth.rs). Ошибок нет:
/// неразборчивый кадр — Obu::Bad.
pub fn parse(text: &str) -> Option<Obu> {
    // Быстрый отсев: кадры других каналов не разбираются здесь второй раз.
    if !text.contains(OBU_CHANNEL) {
        return None;
    }
    let v: Value = serde_json::from_str(text).ok()?;
    if v.get("channel").and_then(Value::as_str) != Some(OBU_CHANNEL) {
        return None;
    }
    let event = v.get("event").and_then(Value::as_str).unwrap_or_default();
    if matches!(event, "subscribe" | "unsubscribe") {
        let stream = v
            .pointer("/payload/0")
            .and_then(Value::as_str)
            .map(str::to_string);
        let error = v
            .get("error")
            .filter(|e| !e.is_null())
            .map(Value::to_string);
        return Some(Obu::Ack {
            event: event.to_string(),
            stream,
            error,
        });
    }
    let r = v.get("result")?;
    Some(frame(r))
}

/// Кадр стакана из result. Ошибок нет: неразборчивый — Obu::Bad.
fn frame(r: &Value) -> Obu {
    let stream = r.get("s").and_then(Value::as_str).map(str::to_string);
    let Some(s) = stream.clone() else {
        return Obu::Bad {
            stream: None,
            reason: "spot.obu без поля s".into(),
        };
    };
    let Some(last) = r.get("u").and_then(Value::as_i64) else {
        return Obu::Bad {
            stream,
            reason: "spot.obu без u".into(),
        };
    };
    let (bids, asks) = match (changes(r, "b"), changes(r, "a")) {
        (Ok(b), Ok(a)) => (b, a),
        (Err(reason), _) | (_, Err(reason)) => return Obu::Bad { stream, reason },
    };
    Obu::Update(Frame {
        stream: s,
        full: r.get("full").and_then(Value::as_bool).unwrap_or(false),
        first: r.get("U").and_then(Value::as_i64),
        last,
        t: r.get("t").and_then(Value::as_i64),
        bids,
        asks,
    })
}

/// Изменения стороны [цена, количество] строками → числа; количество 0 сохраняется (убрать уровень). Ошибка — текст
/// причины (и когда поля нет: в обоих примерах документации b и a есть — отсутствие не толкуем, кадр не разобран).
fn changes(r: &Value, side: &str) -> Result<Vec<PriceLevel>, String> {
    let arr = r
        .get(side)
        .and_then(Value::as_array)
        .ok_or(format!("spot.obu без {side}"))?;
    let num = |x: &Value| x.as_str().and_then(|s| Decimal::from_str(s.trim()).ok());
    arr.iter()
        .map(|l| {
            let p = l.as_array().filter(|p| p.len() >= 2);
            let p = p.ok_or(format!("{side}: уровень не [цена, количество]"))?;
            let price = num(&p[0]).ok_or(format!("{side}: цена не число"))?;
            let qty = num(&p[1]).ok_or(format!("{side}: количество не число"))?;
            Ok(PriceLevel { price, qty })
        })
        .collect()
}

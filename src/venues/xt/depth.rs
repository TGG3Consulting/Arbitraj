//! Файл: venues/xt/depth.rs
//! Что делает: разбор кадров публичного WebSocket XT.com: снимок depth@{symbol},{levels}, изменения
//! depth_update@{symbol}, ответ на subscribe, текст «pong»; разбор снимка REST /v4/public/depth. Уровни читаются без
//! копий строк (заимствование из кадра), в Decimal — только когда нужно (решения 109, 130, урок 13).
//! Кто вызывает: ws_conn.rs, local_book.rs, snapshots.rs, тесты.
//! От чего зависит: core/types, serde.
//! Настройки: нет.
//!
//! Официально (https://doc.xt.com/#websocket_publicpushMessage и далее, `_websocket_public/*.md`):
//! - push {"topic", "event": "<topic>@<args>", "data"}; ответ {"id", "code": 0 успех / 1 отказ, "msg"};
//! - Limited depth (limitDepth.md): data {s, i updateId, t время, a [[цена, количество]], b} — снимок раз в 1000 мс;
//! - Incremental depth (increDepth.md): data {s, fi firstUpdateId «= previous lastUpdateId + 1», i lastUpdateId, a, b} —
//!   раз в 100 мс; «The data in each event is the absolute quantity», «If the quantity is 0, remove the price level»
//!   (orderBook.md);
//! - heartBeat.md: сервер отвечает текстом «pong» на текст «ping»;
//! - REST GET /v4/public/depth (`_market/3depth.md`): result {timestamp, lastUpdateId, bids, asks}.
//!   Порядок уровней не оговорён — сортируем сами (asks по возрастанию цены, bids по убыванию).

use std::borrow::Cow;
use std::str::FromStr;

use rust_decimal::Decimal;
use serde::Deserialize;

use crate::core::types::PriceLevel;

/// Уровень как в кадре: [цена, количество] строками.
pub type RawLevel<'a> = (Cow<'a, str>, Cow<'a, str>);

/// data кадра стакана (снимок или изменения).
#[derive(Deserialize)]
pub struct Data<'a> {
    #[serde(borrow, default)]
    pub s: Option<Cow<'a, str>>,
    /// firstUpdateId — только у depth_update.
    #[serde(default)]
    pub fi: Option<i64>,
    /// updateId снимка или lastUpdateId изменения.
    pub i: i64,
    #[serde(default)]
    pub t: Option<i64>,
    #[serde(borrow, default)]
    pub a: Vec<RawLevel<'a>>,
    #[serde(borrow, default)]
    pub b: Vec<RawLevel<'a>>,
}

/// Кадр целиком: push или ответ на запрос.
#[derive(Deserialize)]
struct Frame<'a> {
    #[serde(borrow, default)]
    topic: Option<Cow<'a, str>>,
    #[serde(borrow, default)]
    data: Option<Data<'a>>,
    #[serde(default)]
    id: Option<serde_json::Value>,
    #[serde(default)]
    code: Option<i64>,
    #[serde(borrow, default)]
    msg: Option<Cow<'a, str>>,
}

/// Что пришло в кадре.
pub enum Msg<'a> {
    /// Снимок depth: символ (поле s), данные.
    Snapshot { symbol: String, data: Data<'a> },
    /// Изменения depth_update: символ, данные.
    Update { symbol: String, data: Data<'a> },
    /// Ответ на запрос без ошибки (id).
    Ack(String),
    /// Отказ запроса: код, текст, id.
    Error { code: i64, msg: String, id: String },
    /// Ответ сервера на ping.
    Pong,
    /// Кадр не разобран — причина.
    Bad(String),
    /// Другое (только в сыром журнале).
    Other,
}

/// Разбирает текстовый кадр. Ошибок нет: неразборчивый — Msg::Bad.
pub fn parse(text: &str) -> Msg<'_> {
    if text.trim() == "pong" {
        return Msg::Pong;
    }
    let f: Frame<'_> = match serde_json::from_str(text) {
        Ok(f) => f,
        Err(e) => return Msg::Bad(format!("кадр не JSON стакана: {e}")),
    };
    if let Some(code) = f.code {
        let id =
            f.id.map(|v| v.as_str().map_or_else(|| v.to_string(), str::to_string))
                .unwrap_or_default();
        if code == 0 {
            return Msg::Ack(id);
        }
        let msg = f.msg.map(|m| m.into_owned()).unwrap_or_default();
        return Msg::Error { code, msg, id };
    }
    let (Some(topic), Some(data)) = (f.topic, f.data) else {
        return Msg::Other;
    };
    let symbol = data.s.as_deref().unwrap_or_default().to_string();
    match topic.as_ref() {
        "depth" => Msg::Snapshot { symbol, data },
        "depth_update" => Msg::Update { symbol, data },
        _ => Msg::Other,
    }
}

/// Что делает: уровни → PriceLevel (нулевые оставляются — у изменений это «убрать уровень»).
/// Принимает: уровни стороны, имя стороны. Возвращает: список. Ошибка — текст причины (не число).
pub fn convert(items: &[RawLevel<'_>], side: &str) -> Result<Vec<PriceLevel>, String> {
    let mut out = Vec::with_capacity(items.len());
    for (p, q) in items {
        let price = Decimal::from_str(p).map_err(|_| format!("{side}: цена «{p}» не число"))?;
        let qty = Decimal::from_str(q).map_err(|_| format!("{side}: количество «{q}» не число"))?;
        out.push(PriceLevel { price, qty });
    }
    Ok(out)
}

/// Что делает: снимок → (bids, asks) без нулевых, отсортированные. Ошибка — текст причины.
pub fn snapshot_levels(d: &Data<'_>) -> Result<(Vec<PriceLevel>, Vec<PriceLevel>), String> {
    let keep = |v: Vec<PriceLevel>| -> Vec<PriceLevel> {
        v.into_iter().filter(|l| l.qty > Decimal::ZERO).collect()
    };
    let mut bids = keep(convert(&d.b, "bids")?);
    let mut asks = keep(convert(&d.a, "asks")?);
    asks.sort_by_key(|a| a.price);
    bids.sort_by_key(|b| std::cmp::Reverse(b.price));
    Ok((bids, asks))
}

/// Снимок REST /v4/public/depth (result).
#[derive(Deserialize)]
pub struct RestDepth<'a> {
    #[serde(rename = "lastUpdateId")]
    pub last_update_id: i64,
    #[serde(borrow, default)]
    pub bids: Vec<RawLevel<'a>>,
    #[serde(borrow, default)]
    pub asks: Vec<RawLevel<'a>>,
}

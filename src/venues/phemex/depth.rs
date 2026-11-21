//! Файл: venues/phemex/depth.rs
//! Что делает: разбор текстовых кадров публичного WebSocket Phemex: ответы на запросы ({error, id, result} — подписка,
//! pong) и кадры стакана (book, snapshot / incremental) — уровни остаются масштабированными целыми (перевод в Decimal
//! — в local_book.rs только для верхних уровней). Один разбор JSON на кадр (урок 13).
//! Кто вызывает: ws_conn.rs, user_stream_conn.rs (ответы), тесты.
//! От чего зависит: num.rs (to_int), serde_json.
//! Настройки: нет.
//!
//! Официально (Spot Websocket API, https://phemex-docs.github.io/#orderbook-message):
//! - ответ: {"error": null, "id": 0, "result": {"status": "success"}} (подписка), «"result": "pong"» (heartbeat);
//!   вид ошибки (error не null) в документации не показан — текст кадра пишется в журнал как есть;
//! - стакан: {"book": {"asks": [[priceEp, qty]], "bids": […]}, "depth", "sequence", "symbol", "timestamp" (нс),
//!   "type": "snapshot" | "incremental"}; «Non-zero qty indicates price level insertion or updation, and qty 0
//!   indicates price level deletion»; «Snapshot messages are published with 60-second interval for client
//!   self-verification».

use serde_json::Value;

use super::num;

/// Уровень стакана в масштабированных целых: (цена Ep, количество Ev базовой монеты).
pub type RawLevel = (i64, i64);

/// Кадр стакана пары.
#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    pub symbol: String,
    pub snapshot: bool,
    pub seq: Option<i64>,
    pub bids: Vec<RawLevel>,
    pub asks: Vec<RawLevel>,
    /// timestamp — наносекунды.
    pub ts_ns: Option<i64>,
}

/// Что пришло в кадре.
#[derive(Debug, Clone, PartialEq)]
pub enum Msg {
    /// Ответ на запрос: id, успех (error = null), текст кадра (для журнала при ошибке).
    Reply {
        id: Option<u64>,
        ok: bool,
        text: String,
    },
    /// Кадр стакана.
    Book(Frame),
    /// Кадр не разобран (символ, если известен, и причина).
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
    if let Some(book) = v.get("book") {
        return book_frame(&v, book);
    }
    if v.get("id").is_some() && (v.get("result").is_some() || v.get("error").is_some()) {
        return Msg::Reply {
            id: v.get("id").and_then(Value::as_u64),
            ok: v.get("error").is_none_or(Value::is_null),
            text: text.to_string(),
        };
    }
    Msg::Other
}

/// Кадр стакана. Ошибок нет: неразборчивый — Msg::Bad.
fn book_frame(v: &Value, book: &Value) -> Msg {
    let symbol = num::text_opt(v, "symbol").unwrap_or_default();
    let snapshot = match v.get("type").and_then(Value::as_str) {
        Some("snapshot") => true,
        Some("incremental") => false,
        other => {
            return Msg::Bad {
                symbol: Some(symbol),
                reason: format!("type = {other:?} (по документации snapshot или incremental)"),
            };
        }
    };
    let (bids, asks) = match (levels(book, "bids"), levels(book, "asks")) {
        (Ok(b), Ok(a)) => (b, a),
        (Err(reason), _) | (_, Err(reason)) => {
            return Msg::Bad {
                symbol: Some(symbol),
                reason,
            };
        }
    };
    let int = |f: &str| {
        v.get(f)
            .and_then(num::to_int)
            .and_then(|i| i64::try_from(i).ok())
    };
    Msg::Book(Frame {
        symbol,
        snapshot,
        seq: int("sequence"),
        bids,
        asks,
        ts_ns: int("timestamp"),
    })
}

/// Уровни [priceEp, qty] (целые) → RawLevel; количество 0 остаётся (удаление уровня). Ошибка — текст причины.
pub fn levels(book: &Value, side: &str) -> Result<Vec<RawLevel>, String> {
    let Some(arr) = book.get(side).and_then(Value::as_array) else {
        return Err(format!("нет {side}"));
    };
    let mut out = Vec::with_capacity(arr.len());
    for l in arr {
        let p = l
            .as_array()
            .filter(|p| p.len() >= 2)
            .ok_or_else(|| format!("{side}: уровень не [priceEp, qty]"))?;
        let int = |x: &Value, what: &str| {
            num::to_int(x)
                .and_then(|i| i64::try_from(i).ok())
                .filter(|i| *i >= 0)
                .ok_or_else(|| format!("{side}: {what} {x} не целое ≥ 0"))
        };
        out.push((int(&p[0], "цена")?, int(&p[1], "количество")?));
    }
    Ok(out)
}

//! Файл: venues/bybit/depth.rs
//! Что делает: разбор текстовых кадров публичного WebSocket спота Bybit V5: ответы на op (subscribe, ping) и кадры
//! стакана orderbook.{depth}.{symbol} (snapshot / delta). Разбор — типизированный serde со ссылками на текст кадра
//! (без копий строк уровней — урок 13): строки цены и количества переводятся в Decimal один раз в local_book.rs.
//! Кто вызывает: ws_conn.rs, feed.rs (ответы op), tap.rs (разбор записанного кадра), user_stream_conn.rs (ответы op).
//! От чего зависит: serde, serde_json.
//! Настройки: нет.
//!
//! Официальная документация:
//! - Orderbook https://bybit-exchange.github.io/docs/v5/websocket/public/orderbook — topic, type («snapshot», «delta»),
//!   ts, data.s, data.b / data.a (массивы [цена, количество] строками; количество 0 — уровень удалить), data.u
//!   («Update ID»; «u=1 … snapshot data due to the restart of the service»), data.seq, cts. В примечании: у спота «ts»
//!   приходит строкой ("1772694601512"), в таблице — number: читаются оба вида.
//! - Connect https://bybit-exchange.github.io/docs/v5/ws/connect — ответ подписки спота {"success":true,"ret_msg":
//!   "subscribe","conn_id":…,"req_id":…,"op":"subscribe"}, pong {"success":true,"ret_msg":"pong",…,"op":"ping"}.

use serde::Deserialize;
use serde_json::Value;

/// Кадр стакана или ответ на op. Поля, которых нет, — None.
#[derive(Deserialize)]
pub struct Frame<'a> {
    #[serde(borrow, default)]
    pub topic: Option<&'a str>,
    #[serde(rename = "type", borrow, default)]
    pub kind: Option<&'a str>,
    #[serde(default)]
    pub ts: Option<Value>,
    #[serde(borrow, default)]
    pub data: Option<BookData<'a>>,
    #[serde(borrow, default)]
    pub op: Option<&'a str>,
    #[serde(default)]
    pub success: Option<bool>,
    #[serde(default)]
    pub ret_msg: Option<String>,
    #[serde(default)]
    pub req_id: Option<String>,
}

/// Только тема кадра (кадр не стакан: data другого вида).
#[derive(Deserialize)]
struct Head<'a> {
    #[serde(borrow, default)]
    topic: Option<&'a str>,
}

/// Содержимое кадра стакана.
#[derive(Deserialize)]
pub struct BookData<'a> {
    #[serde(borrow)]
    pub s: &'a str,
    #[serde(borrow, default)]
    pub b: Vec<[&'a str; 2]>,
    #[serde(borrow, default)]
    pub a: Vec<[&'a str; 2]>,
    pub u: Value,
    #[serde(default)]
    pub seq: Option<Value>,
}

/// Ответ на op (подписка, ping, auth) — без поля data (у ленты сделок data — массив, разбирать не нужно).
#[derive(Deserialize)]
pub struct Ack {
    #[serde(default)]
    pub op: Option<String>,
    #[serde(default)]
    pub success: Option<bool>,
    #[serde(default)]
    pub ret_msg: Option<String>,
    #[serde(default)]
    pub req_id: Option<String>,
}

/// Что пришло в кадре.
pub enum Msg<'a> {
    /// Стакан: символ, снимок ли, номер u, время площадки (мс), уровни строками.
    Book {
        symbol: &'a str,
        snapshot: bool,
        u: i64,
        ts: Option<i64>,
        bids: Vec<[&'a str; 2]>,
        asks: Vec<[&'a str; 2]>,
    },
    /// Ответ на op: какой op, успех, текст, req_id.
    Ack {
        op: String,
        success: bool,
        msg: String,
        req_id: Option<String>,
    },
    /// Кадр не разобран — причина.
    Bad(String),
    /// Другое (только в сырых сообщениях).
    Other,
}

/// Число из значения JSON: число или строка с числом. Ошибок нет: иначе None.
pub fn int_of(v: &Value) -> Option<i64> {
    match v {
        Value::Number(n) => n.to_string().parse().ok(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

/// Разбирает текстовый кадр рынка. Ошибок нет: неразборчивый кадр — Msg::Bad.
pub fn parse(text: &str) -> Msg<'_> {
    let f: Frame<'_> = match serde_json::from_str(text) {
        Ok(f) => f,
        // Не стакан (у publicTrade data — массив; живая запись 2026-11-04): разбор заголовка без data, второй разбор
        // — только на ошибке, кадры стаканов разбираются один раз.
        Err(e) => {
            return match serde_json::from_str::<Head<'_>>(text) {
                Ok(h) if h.topic.is_some_and(|t| !t.starts_with("orderbook.")) => Msg::Other,
                _ => Msg::Bad(format!("JSON: {e}")),
            };
        }
    };
    if let Some(op) = f.op {
        return Msg::Ack {
            op: op.to_string(),
            success: f.success.unwrap_or(false),
            msg: f.ret_msg.unwrap_or_default(),
            req_id: f.req_id,
        };
    }
    let (Some(topic), Some(data)) = (f.topic, f.data) else {
        return Msg::Other;
    };
    if !topic.starts_with("orderbook.") {
        return Msg::Other;
    }
    let snapshot = match f.kind {
        Some("snapshot") => true,
        Some("delta") => false,
        other => return Msg::Bad(format!("{topic}: неизвестный type {other:?}")),
    };
    let Some(u) = int_of(&data.u) else {
        return Msg::Bad(format!("{topic}: u не число"));
    };
    Msg::Book {
        symbol: data.s,
        snapshot,
        u,
        ts: f.ts.as_ref().and_then(int_of),
        bids: data.b,
        asks: data.a,
    }
}

/// Разбирает только ответ на op (соединения окна и приватный поток). Ошибок нет: не op — None.
pub fn ack(text: &str) -> Option<Ack> {
    serde_json::from_str::<Ack>(text)
        .ok()
        .filter(|a| a.op.is_some())
}

/// Канал стакана: «orderbook.{depth}.{symbol}» (официальная документация).
pub fn channel(levels: u64, symbol: &str) -> String {
    format!("orderbook.{levels}.{symbol}")
}

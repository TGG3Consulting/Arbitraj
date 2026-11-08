//! Файл: venues/okx/ws_req.rs
//! Что делает: подключение WebSocket OKX и запросы подписки (op = subscribe / unsubscribe) пачками не длиннее 64 КБ —
//! общие для соединений стаканов (ws_conn.rs) и потока аккаунта (user_stream_conn.rs); переподписка одной пары
//! (unsubscribe + subscribe — разрыв prevSeqId и команда сторожа пар, решение 202).
//! Кто вызывает: ws_conn.rs, user_stream_conn.rs, тесты.
//! От чего зависит: tokio-tungstenite, serde_json.
//! Настройки: нет (адрес и срок передаёт вызывающий).
//!
//! Официально (https://www.okx.com/docs-v5/en/, «Overview → WebSocket»): «the total length of multiple channels cannot
//! exceed 64 KB»; «The total number of 'subscribe'/'unsubscribe'/'login' requests per connection is limited to 480 times
//! per hour»; «Overview → WebSocket → Unsubscribe»: «{"op": "unsubscribe", "args": ["<SubscriptionTopic>"]}», пример
//! {"op":"unsubscribe","args":[{"channel":"tickers","instId":"BTC-USDT"}]}.

use std::time::Duration;

use futures_util::SinkExt;
use futures_util::stream::SplitSink;
use serde_json::json;
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

/// Предел длины одного запроса подписки — официально «cannot exceed 64 KB».
pub const MAX_REQ_BYTES: usize = 64 * 1024;
/// Запросов subscribe/unsubscribe/login на соединение в час — официально «480 times per hour».
pub const MAX_OPS_PER_HOUR: usize = 480;

/// Открытое соединение.
pub type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// Что делает: подключение. Принимает: адрес, срок. Ошибка — текст.
pub async fn open(url: &str, timeout: Duration) -> Result<Ws, String> {
    match tokio::time::timeout(timeout, tokio_tungstenite::connect_async(url)).await {
        Err(_) => Err("не удалось подключиться за отведённое время".into()),
        Ok(Err(e)) => Err(format!("подключение к {url}: {e}")),
        Ok(Ok((ws, _))) => Ok(ws),
    }
}

/// Запросы подписки (op = subscribe / unsubscribe): аргументы пачками, чтобы запрос был не длиннее 64 КБ. Ошибок нет.
pub fn requests(op: &str, channel: &str, insts: &[&str]) -> Vec<String> {
    let mut out = Vec::new();
    let mut args: Vec<serde_json::Value> = Vec::new();
    // Длина «обёртки» запроса без аргументов — считается, а не задаётся числом.
    let envelope = json!({"op": op, "args": []}).to_string().len();
    let mut len = envelope;
    for i in insts {
        let a = json!({"channel": channel, "instId": i});
        let n = a.to_string().len() + 1;
        if !args.is_empty() && len + n > MAX_REQ_BYTES {
            out.push(json!({"op": op, "args": std::mem::take(&mut args)}).to_string());
            len = envelope;
        }
        len += n;
        args.push(a);
    }
    if !args.is_empty() {
        out.push(json!({"op": op, "args": args}).to_string());
    }
    out
}

/// Разъём на запись соединения.
pub type Sink = SplitSink<Ws, Message>;

/// Переподписка одной пары: unsubscribe, затем subscribe канала пары в этом же соединении (оба — в счёт «480 times per
/// hour»). Принимает: разъём, канал, instId, счётчик запросов сессии. Ошибка — текст (запрос не отправлен).
pub async fn resub(
    sink: &mut Sink,
    channel: &str,
    inst: &str,
    ops: &mut usize,
) -> Result<(), String> {
    for op in ["unsubscribe", "subscribe"] {
        *ops += 1;
        let r = requests(op, channel, &[inst]).concat();
        sink.send(Message::Text(r.into()))
            .await
            .map_err(|e| format!("отправка {op}: {e}"))?;
    }
    Ok(())
}

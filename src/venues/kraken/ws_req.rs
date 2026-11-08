//! Файл: venues/kraken/ws_req.rs
//! Что делает: подключение WebSocket v2 Kraken через общий ограничитель попыток (на все соединения адаптера с этого IP)
//! и запросы: подписка / отписка канала book пачками символов, ping, подписка balances с токеном; переподписка одной
//! пары (unsubscribe + subscribe — разрыв чек-суммы и команда сторожа пар, решение 202).
//! Кто вызывает: ws_conn.rs, user_stream_conn.rs, тесты.
//! От чего зависит: tokio-tungstenite, serde_json.
//! Настройки: нет (адрес, срок и размер пачки передаёт вызывающий).
//!
//! Официально:
//! - https://docs.kraken.com/exchange/guides/websockets/introduction.md — Cloudflare: «approximately 150 attempts per
//!   rolling 10 minutes» на IP, сверх — бан IP на 10 минут: попытки не чаще одной на 600/150 = 4 с;
//! - https://docs.kraken.com/exchange/api-reference/spot-websocket-v2/book.md — {"method":"subscribe","params":{"channel":
//!   "book","symbol":[…],"depth":N,"snapshot":true},"req_id":…}; отписка — method unsubscribe, те же channel, symbol,
//!   depth; «multiple symbols in one request», подтверждение — по символу;
//! - https://docs.kraken.com/exchange/api-reference/spot-websocket.md — {"method":"ping","req_id":…} → {"method":"pong"};
//! - https://docs.kraken.com/exchange/api-reference/spot-websocket-v2/balances.md — подписка balances с params.token.

use std::sync::Mutex;
use std::time::Duration;

use futures_util::SinkExt;
use futures_util::stream::SplitSink;
use serde_json::{Value, json};
use tokio::net::TcpStream;
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

/// Попыток подключения на IP за окно — официально «approximately 150 attempts per rolling 10 minutes».
pub const CF_ATTEMPTS: u64 = 150;
/// Окно Cloudflare, секунд — «rolling 10 minutes».
pub const CF_WINDOW_S: u64 = 600;

/// Когда можно следующую попытку подключения (общая на адаптер).
static NEXT: Mutex<Option<Instant>> = Mutex::new(None);

/// Открытое соединение.
pub type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// Что делает: занимает слот попытки подключения — ждёт, пока не пройдёт CF_WINDOW_S / CF_ATTEMPTS с прошлой попытки.
/// Ошибок нет.
pub async fn connect_slot() {
    let gap = Duration::from_millis(CF_WINDOW_S * 1000 / CF_ATTEMPTS);
    let at = {
        let mut g = NEXT.lock().unwrap_or_else(|p| p.into_inner());
        let now = Instant::now();
        let at = g.map_or(now, |n| n.max(now));
        *g = Some(at + gap);
        at
    };
    tokio::time::sleep_until(at).await;
}

/// Что делает: подключение (после слота попытки). Принимает: адрес, срок. Ошибка — текст.
pub async fn open(url: &str, timeout: Duration) -> Result<Ws, String> {
    connect_slot().await;
    match tokio::time::timeout(timeout, tokio_tungstenite::connect_async(url)).await {
        Err(_) => Err("не удалось подключиться за отведённое время".into()),
        Ok(Err(e)) => Err(format!("подключение к {url}: {e}")),
        Ok(Ok((ws, _))) => Ok(ws),
    }
}

/// Запросы subscribe / unsubscribe канала book: символы пачками по per_msg. Принимает: метод, глубину, символы,
/// размер пачки, первый req_id. Ошибок нет.
pub fn book_requests(
    method: &str,
    depth: u64,
    symbols: &[&str],
    per_msg: usize,
    req_id: u64,
) -> Vec<String> {
    symbols
        .chunks(per_msg.max(1))
        .enumerate()
        .map(|(i, c)| {
            let mut params = json!({"channel": "book", "symbol": c, "depth": depth});
            if method == "subscribe" {
                params["snapshot"] = Value::Bool(true);
            }
            json!({"method": method, "params": params, "req_id": req_id + i as u64}).to_string()
        })
        .collect()
}

/// Разъём на запись соединения.
pub type Sink = SplitSink<Ws, Message>;

/// Переподписка одной пары канала book: unsubscribe (те же channel, symbol, depth — «The number of price levels to be
/// unsubscribed»), затем subscribe со снимком, в этом же соединении. Принимает: разъём, глубину, имя пары WebSocket,
/// счётчик req_id. Ошибка — текст (запрос не отправлен).
pub async fn resub(
    sink: &mut Sink,
    depth: u64,
    wsname: &str,
    req_id: &mut u64,
) -> Result<(), String> {
    for method in ["unsubscribe", "subscribe"] {
        *req_id += 1;
        let r = book_requests(method, depth, &[wsname], 1, *req_id).concat();
        sink.send(Message::Text(r.into()))
            .await
            .map_err(|e| format!("отправка {method}: {e}"))?;
    }
    Ok(())
}

/// Запрос ping. Принимает: req_id. Ошибок нет.
pub fn ping(req_id: u64) -> String {
    json!({"method": "ping", "req_id": req_id}).to_string()
}

/// Подписка balances приватного соединения (содержит токен — в журнал не писать). Принимает: токен. Ошибок нет.
/// snapshot = false: снимок балансов ядру не нужен (изменения извне — только обновления).
pub fn balances(token: &str) -> String {
    json!({"method": "subscribe", "params": {"channel": "balances", "token": token, "snapshot": false}}).to_string()
}

//! Файл: venues/bsc_dex/node/ws_task.rs
//! Что делает: задача одного соединения WebSocket с узлом BSC — пишет запросы, читает ответы и уведомления,
//! раздаёт ответы по id и уведомления eth_subscription по id подписки, пишет сырые сообщения на диск.
//! Решение 98: тишина считается от последнего кадра от узла; для тихих соединений бот шлёт Ping раз в
//! venue.ws_ping_ms (узел отвечает Pong — RFC 6455), так живое соединение без событий не рвётся.
//! Тишина дольше venue.ws_silence_ms, ошибка чтения или закрытие сервером — конец соединения:
//! все ожидающие запросы получают ошибку, все потоки подписок закрываются (владельцы переподключаются).
//! Кто вызывает: node/ws.rs (Conn::open).
//! От чего зависит: ctx.rs (журнал, сырые сообщения), node/jsonrpc.rs, errors.rs, tokio-tungstenite.
//! Настройки: приходят из ws.rs (тишина).
//!
//! Уведомление подписки: {"jsonrpc":"2.0","method":"eth_subscription","params":{"subscription":id,"result":…}} —
//! https://github.com/ethereum/execution-apis/blob/16caee274b0d4fb7a0316f294029adac88a75a09/src/eth/subscribe.yaml#L1-L60 ;
//! отписка eth_unsubscribe(id) — там же. Ping/Pong уровня WebSocket отвечает tokio-tungstenite (RFC 6455, 5.5.2).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use alloy::rpc::json_rpc::Response;
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::value::RawValue;
use serde_json::{Value, json};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

use crate::gateway::errors::GatewayResult;

use super::super::ctx::{Ctx, ID};
use super::super::errors::Op;
use super::jsonrpc;
use super::ws::SOURCE;

/// Ответ на команду: сырой result или ошибка.
pub type Reply = oneshot::Sender<GatewayResult<Box<RawValue>>>;

/// Команда задаче соединения.
pub enum Cmd {
    /// Запрос с ответом.
    Request {
        method: String,
        params: Value,
        reply: Reply,
    },
    /// eth_subscribe: ответ — id подписки, уведомления — в events.
    Subscribe {
        params: Value,
        events: mpsc::UnboundedSender<Box<RawValue>>,
        reply: Reply,
    },
}

/// Что ждёт ответа по id запроса.
enum Pending {
    Req(Reply),
    Sub(Reply, mpsc::UnboundedSender<Box<RawValue>>),
}

/// Уведомление подписки (eth_subscription).
#[derive(Deserialize)]
struct Notification {
    method: String,
    params: NotifParams,
}

#[derive(Deserialize)]
struct NotifParams {
    subscription: String,
    result: Box<RawValue>,
}

/// Работает до конца соединения; причину пишет в журнал и в `end`.
/// Принимает: окружение, имя, поток WebSocket, команды, предел тишины, ячейку причины. Ошибок не возвращает.
pub async fn run(
    ctx: Ctx,
    name: String,
    ws: WebSocketStream<MaybeTlsStream<TcpStream>>,
    mut cmds: mpsc::UnboundedReceiver<Cmd>,
    silence: Duration,
    ping: Option<Duration>,
    end: Arc<Mutex<Option<String>>>,
) {
    let (mut sink, mut stream) = ws.split();
    let mut pending: HashMap<u64, Pending> = HashMap::new();
    let mut subs: HashMap<String, mpsc::UnboundedSender<Box<RawValue>>> = HashMap::new();
    let mut next_id: u64 = 1;
    // Решение 98: тишина отсчитывается от ПОСЛЕДНЕГО КАДРА ОТ УЗЛА (любого, вкл. Pong), а не от прохода цикла: раньше
    // таймер создавался заново на каждой итерации и сбрасывался нашими же запросами. Живость тихого соединения
    // (пачка редко торгуемых пулов, «новые пулы») подтверждается нашим Ping (venue.ws_ping_ms): узел обязан ответить
    // Pong (RFC 6455, 5.5.2–5.5.3). Нет ответа дольше venue.ws_silence_ms — соединение мёртвое, как и раньше.
    let mut last_rx = tokio::time::Instant::now();
    let mut pinger = ping.map(|d| tokio::time::interval_at(tokio::time::Instant::now() + d, d));
    let reason = loop {
        tokio::select! {
            _ = tokio::time::sleep_until(last_rx + silence) => {
                break format!("нет сообщений дольше {} мс", silence.as_millis());
            }
            _ = async {
                match pinger.as_mut() {
                    Some(i) => { i.tick().await; }
                    None => std::future::pending::<()>().await,
                }
            } => {
                if let Err(e) = sink.send(Message::Ping(Vec::new().into())).await {
                    break format!("отправка ping: {e}");
                }
            }
            cmd = cmds.recv() => {
                // Владелец удалил соединение — тихо заканчиваем.
                let Some(cmd) = cmd else { break "закрыто владельцем".to_string() };
                let id = next_id;
                next_id += 1;
                let (method, params, p) = match cmd {
                    Cmd::Request { method, params, reply } => (method, params, Pending::Req(reply)),
                    Cmd::Subscribe { params, events, reply } => ("eth_subscribe".into(), params, Pending::Sub(reply, events)),
                };
                pending.insert(id, p);
                let body = jsonrpc::request(id, &method, &params).to_string();
                if let Err(e) = sink.send(Message::Text(body.into())).await {
                    break format!("отправка запроса: {e}");
                }
            }
            next = stream.next() => {
                let frame = match next {
                    None => break "соединение закрыто сервером".to_string(),
                    Some(Err(e)) => break format!("ошибка чтения: {e}"),
                    Some(Ok(f)) => f,
                };
                // Любой кадр от узла (текст, Pong, Ping) — узел жив.
                last_rx = tokio::time::Instant::now();
                let text = match frame {
                    Message::Text(t) => t.to_string(),
                    Message::Close(c) => break format!("сервер закрыл соединение: {c:?}"),
                    _ => continue,
                };
                ctx.raw.write(ID, &name, &text);
                if let Some(gone) = dispatch(&text, &mut pending, &mut subs) {
                    // Получатель подписки больше не слушает — отписываемся у узла, чтобы не тратить квоту.
                    let id = next_id;
                    next_id += 1;
                    let body = jsonrpc::request(id, "eth_unsubscribe", &json!([gone])).to_string();
                    if let Err(e) = sink.send(Message::Text(body.into())).await {
                        break format!("отправка отписки: {e}");
                    }
                }
            }
        }
    };
    // Причину пишет в журнал владелец соединения (по end_reason): слушатели пулов — одной записью на площадку
    // (pools/link.rs), а не по записи на каждое соединение.
    *end.lock().unwrap_or_else(|p| p.into_inner()) = Some(reason);
    // pending и subs удаляются здесь: ожидающие получают «закрыто», потоки подписок закрываются.
}

/// Раздаёт одно сообщение: уведомление — в поток подписки, ответ — ожидающему по id.
/// Принимает: текст, ожидающие, подписки. Возвращает: id подписки, получатель которой ушёл (нужна отписка).
fn dispatch(
    text: &str,
    pending: &mut HashMap<u64, Pending>,
    subs: &mut HashMap<String, mpsc::UnboundedSender<Box<RawValue>>>,
) -> Option<String> {
    if let Ok(n) = serde_json::from_str::<Notification>(text) {
        if n.method != "eth_subscription" {
            return None;
        }
        let sub = n.params.subscription;
        let alive = subs.get(&sub).map(|tx| tx.send(n.params.result).is_ok());
        return match alive {
            Some(false) => {
                subs.remove(&sub);
                Some(sub)
            }
            _ => None,
        };
    }
    let Ok(resp) = serde_json::from_str::<Response>(text) else {
        tracing::warn!(venue = ID, "узел прислал непонятное сообщение");
        return None;
    };
    let (id, res) = jsonrpc::parse(resp, SOURCE, Op::Read, text);
    match id.and_then(|i| pending.remove(&i)) {
        Some(Pending::Req(reply)) => {
            let _ = reply.send(res);
        }
        Some(Pending::Sub(reply, events)) => {
            if let Ok(raw) = &res
                && let Ok(sub_id) = serde_json::from_str::<String>(raw.get())
            {
                subs.insert(sub_id, events);
            }
            let _ = reply.send(res);
        }
        None => {}
    }
    None
}

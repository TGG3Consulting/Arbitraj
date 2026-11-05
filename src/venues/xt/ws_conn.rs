//! Файл: venues/xt/ws_conn.rs
//! Что делает: одна сессия публичного WebSocket XT.com: подключение, subscribe пачками по venue.ws_streams_per_msg тем
//! с паузой, текст «ping» раз в venue.ws_ping_ms, тишина дольше venue.ws_silence_ms — обрыв; кадры стаканов и итоги
//! снимков REST — в ws_book.rs (память и ядро); отказ подписки — стаканы её пар недостоверны.
//! Кто вызывает: ws.rs.
//! От чего зависит: depth.rs, ws_book.rs, snapshots.rs, book.rs, ws.rs (Cfg, End), ctx.rs, tokio-tungstenite.
//! Настройки: из ws.rs (Cfg); raw.book_frames [xt] — писать ли кадры стаканов в сырой журнал.
//!
//! Официально (https://doc.xt.com/#websocket_publicrequestFormat, responseFormat, heartBeat):
//! {"method":"subscribe","params":["depth_update@btc_usdt", …],"id":"<id>"} → {"id","code":0,"msg":"success"};
//! code 1 — отказ. Клиент шлёт текст «ping», сервер отвечает «pong». Темы — «{topic}@{arg},{arg}» (subscribeParam.md).

use std::collections::HashMap;

use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::Message;

use crate::core::types::Asset;
use crate::gateway::events::MarketEvent;
use crate::storage::journal::Level;

use super::book::Books;
use super::ctx::{Ctx, ID};
use super::depth::{self, Msg};
use super::ws::{Cfg, End, Stream, invalidate};
use super::ws_book::Conn;

/// Имя темы стакана пары: «depth@<symbol>,<levels>» или «depth_update@<symbol>» (символ — как в справочнике). Ошибок нет.
pub fn topic(symbol: &str, cfg: &Cfg) -> String {
    match cfg.stream {
        Stream::Depth => format!("depth@{symbol},{}", cfg.levels),
        Stream::Update => format!("depth_update@{symbol}"),
    }
}

/// Сессия: работает до обрыва. Возвращает причину окончания.
pub async fn session(
    ctx: &Ctx,
    name: &str,
    assets: &[(Asset, String)],
    books: &Books,
    out: &mpsc::Sender<MarketEvent>,
    cfg: Cfg,
) -> End {
    let connect = tokio::time::timeout(
        cfg.connect_timeout,
        tokio_tungstenite::connect_async(cfg.url.as_str()),
    )
    .await;
    let ws = match connect {
        Err(_) => return End::Broken("не удалось подключиться за отведённое время".into()),
        Ok(Err(e)) => return End::Broken(format!("подключение: {e}")),
        Ok(Ok((ws, _))) => ws,
    };
    let (mut sink, mut stream) = ws.split();
    let names: Vec<String> = assets.iter().map(|(_, s)| topic(s, &cfg)).collect();
    let mut batches: Vec<Vec<String>> = names.chunks(cfg.per_msg).map(<[_]>::to_vec).collect();
    batches.reverse();
    // id запроса → его темы (отказ подписки снимает стаканы этих пар).
    let mut sent: HashMap<String, Vec<String>> = HashMap::new();
    let mut next_id = 0u64;
    let mut sub_tick = tokio::time::interval(cfg.sub_gap);
    let mut ping_tick = tokio::time::interval_at(Instant::now() + cfg.ping, cfg.ping);
    let (snap_tx, mut snap_rx) = mpsc::channel(assets.len().max(1));
    let mut conn = Conn::new(ctx, name, assets, books, out, &cfg, snap_tx);
    let mut last_rx = Instant::now();
    loop {
        tokio::select! {
            _ = sub_tick.tick(), if !batches.is_empty() => {
                let Some(params) = batches.pop() else { continue };
                next_id += 1;
                let sub = json!({ "method": "subscribe", "params": params, "id": next_id.to_string() });
                if let Err(e) = sink.send(Message::Text(sub.to_string().into())).await {
                    return End::Broken(format!("отправка подписки: {e}"));
                }
                sent.insert(next_id.to_string(), params);
            }
            _ = ping_tick.tick() => {
                if let Err(e) = sink.send(Message::Text("ping".into())).await {
                    return End::Broken(format!("отправка ping: {e}"));
                }
            }
            _ = tokio::time::sleep_until(last_rx + cfg.silence) => {
                return End::Broken(format!("нет кадров дольше venue.ws_silence_ms ({} мс)", cfg.silence.as_millis()));
            }
            Some(done) = snap_rx.recv() => {
                if !conn.on_snapshot(done).await {
                    return End::Done;
                }
            }
            next = stream.next() => {
                let frame = match next {
                    None => return End::Broken("соединение закрыто сервером".into()),
                    Some(Err(e)) => return End::Broken(format!("ошибка чтения: {e}")),
                    Some(Ok(f)) => f,
                };
                last_rx = Instant::now();
                conn.alive();
                let t = match frame {
                    Message::Text(t) => t,
                    Message::Ping(p) => {
                        if let Err(e) = sink.send(Message::Pong(p)).await {
                            return End::Broken(format!("отправка Pong: {e}"));
                        }
                        continue;
                    }
                    Message::Close(c) => return End::Broken(format!("сервер закрыл соединение: {c:?}")),
                    _ => continue,
                };
                let msg = depth::parse(&t);
                let is_book = matches!(msg, Msg::Snapshot { .. } | Msg::Update { .. } | Msg::Pong);
                if !is_book || matches!(ctx.flag("raw.book_frames"), Ok(true)) {
                    ctx.raw.write(ID, name, &t);
                }
                match msg {
                    Msg::Snapshot { symbol, data } => {
                        if !conn.on_depth(&symbol, &data).await {
                            return End::Done;
                        }
                    }
                    Msg::Update { symbol, data } => {
                        if !conn.on_update(&symbol, &data).await {
                            return End::Done;
                        }
                    }
                    Msg::Error { code, msg, id } => {
                        for s in sent.get(&id).into_iter().flatten() {
                            let sym = s.split_once('@').map_or("", |(_, r)| r);
                            let sym = sym.split(',').next().unwrap_or_default();
                            if let Some(a) = conn.asset(sym) {
                                invalidate(ctx, books, out, &a.ticker);
                            }
                        }
                        let m = format!("WebSocket {name}: отказ подписки id {id}, код {code}: {msg}");
                        ctx.log(Level::Error, "venue.ws", m).await;
                    }
                    Msg::Bad(why) => {
                        let m = format!("WebSocket {name}: кадр не разобран: {why}");
                        ctx.log(Level::Error, "venue.ws", m).await;
                    }
                    Msg::Ack(_) | Msg::Pong | Msg::Other => {}
                }
            }
        }
    }
}

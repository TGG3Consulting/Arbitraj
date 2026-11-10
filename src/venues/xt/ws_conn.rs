//! Файл: venues/xt/ws_conn.rs
//! Что делает: одна сессия публичного WebSocket XT.com: подключение, subscribe пачками по venue.ws_streams_per_msg тем
//! с паузой, текст «ping» раз в venue.ws_ping_ms, тишина дольше venue.ws_silence_ms — обрыв; кадры стаканов и итоги
//! снимков REST — в ws_book.rs (память и ядро); отказ подписки — стаканы её пар недостоверны.
//! Кто вызывает: ws.rs.
//! От чего зависит: depth.rs, ws_book.rs, snapshots.rs, book.rs, ws.rs (Cfg, End), ctx.rs, gateway/feed_reg.rs,
//! tokio-tungstenite.
//! Настройки: из ws.rs (Cfg); raw.book_frames [xt] — писать ли кадры стаканов в сырой журнал.
//!
//! Официально (https://doc.xt.com/#websocket_publicrequestFormat, responseFormat, heartBeat):
//! {"method":"subscribe","params":["depth_update@btc_usdt", …],"id":"<id>"} → {"id","code":0,"msg":"success"};
//! code 1 — отказ. Клиент шлёт текст «ping», сервер отвечает «pong». Темы — «{topic}@{arg},{arg}» (subscribeParam.md).
//! Решение 202 (для всех бирж, 08.11): каждый кадр отмечается в учёте свежести (gateway/feed_reg.rs), кадр стакана — по
//! паре (и снимок с тем же updateId тоже); depth_update — с номером локального стакана (решение 213, ws_book.rs
//! version), как и итог снимка REST, ставший стаканом пары. Команды сторожа ядра: «переподписать пару» —
//! {"method":"unsubscribe","params":[тема],"id"}, затем {"method":"subscribe","params":[тема],"id"} в этом же соединении
//! (https://doc.xt.com/#websocket_publicrequestFormat, `_websocket_public/requestFormat.md`: unsubscribe), соседи не
//! трогаются; отказ подписки — стакан пары недостоверен (как у первой подписки); «переподключить соединение» — сессия
//! заканчивается, ws.rs подключается заново с теми же парами. Дополнение 10.11: при переподписке пары — сразу снимок
//! REST (ws_book.rs resub); снимок, ставший стаканом пары, отмечается в учёте свежести как кадр пары.

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
use crate::gateway::feed_health::Cmd;
use crate::gateway::feed_reg::ConnHandle;

/// Учёт свежести соединения и канал команд сторожа (решение 202).
pub struct Watch<'a> {
    pub feed: &'a ConnHandle,
    pub cmd: &'a mut mpsc::UnboundedReceiver<Cmd>,
}

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
    w: Watch<'_>,
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
    w.feed.session_started();
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
            Some(c) = w.cmd.recv() => match c {
                Cmd::Reconnect => return End::Broken("сторож пар: переподключение соединения (решение 202)".into()),
                Cmd::Resub(ticker) => {
                    let Some((_, sym)) = assets.iter().find(|(a, _)| a.ticker == ticker) else { continue };
                    let t = topic(sym, &cfg);
                    for method in ["unsubscribe", "subscribe"] {
                        next_id += 1;
                        let m = json!({ "method": method, "params": [t], "id": next_id.to_string() });
                        if let Err(e) = sink.send(Message::Text(m.to_string().into())).await {
                            return End::Broken(format!("отправка {method} (сторож пар): {e}"));
                        }
                    }
                    // Отказ подписки — стакан пары недостоверен (разбор Msg::Error ниже).
                    sent.insert(next_id.to_string(), vec![t]);
                    conn.resub(sym);
                    w.feed.resubscribed(&ticker);
                }
            },
            Some(done) = snap_rx.recv() => {
                let sym = done.0.clone();
                let (open, fresh) = conn.on_snapshot(done).await;
                if !open {
                    return End::Done;
                }
                // Снимок REST стал стаканом пары (решение 213): номер стакана — lastUpdateId снимка (depth_update);
                // у канала depth номера нет (дополнение 10.11 — снимок при переподписке).
                if let (true, Some(a)) = (fresh, conn.asset(&sym)) {
                    w.feed.frame_ver(&a.ticker, conn.version(&sym), None);
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
                w.feed.alive();
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
                        if let Some(a) = conn.asset(&symbol) {
                            w.feed.frame(&a.ticker);
                        }
                        if !conn.on_depth(&symbol, &data).await {
                            return End::Done;
                        }
                    }
                    Msg::Update { symbol, data } => {
                        if !conn.on_update(&symbol, &data).await {
                            return End::Done;
                        }
                        if let Some(a) = conn.asset(&symbol) {
                            w.feed.frame_ver(&a.ticker, conn.version(&symbol), None);
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

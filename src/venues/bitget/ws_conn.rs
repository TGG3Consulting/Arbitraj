//! Файл: venues/bitget/ws_conn.rs
//! Что делает: одна сессия WebSocket рынка Bitget: подключение, подписки пачками (каждое сообщение ≤ 4096 байт) с паузой
//! venue.ws_subs_per_sec, свой "ping" раз в venue.ws_ping_ms, тишина дольше venue.ws_silence_ms — обрыв; снимки стакана
//! → память и ядру (без ожидания, try_send); ошибка по паре или неразборчивый снимок → стакан пары недостоверен.
//! Сырые кадры стакана — в журнал только при raw.book_frames = да (решение 109).
//! Кто вызывает: ws.rs.
//! От чего зависит: depth.rs, book.rs, ctx.rs, gateway/feed_reg.rs, tokio-tungstenite.
//! Настройки: приходят из ws.rs (Cfg); raw.book_frames [bitget].
//!
//! Решение 109: снимок с тем же или меньшим seq, что прежний, ядру не отдаётся — стакан не менялся (seq «increases when
//! the order book is updated», Depth Channel); время кадра пары обновляется (book.rs, touch). Сравнение по номеру, без
//! копий уровней (решения 130, 131). Нет seq в кадре — снимок отдаётся всегда.
//! Официально (WebSocket intro): подписка {"op":"subscribe","args":[{"instType":"SPOT","channel":"books5","instId":…}]};
//! «The total length of multiple channels cannot exceed 4096 bytes» в одном запросе; «240 subscription requests/Hour/
//! connection, Max 1000 channel subscription/connection» — подписка шлётся только при подключении (урок 14).
//! Решение 202 (для всех бирж, 08.11): каждый кадр отмечается в учёте свежести (gateway/feed_reg.rs) — кадр стакана по
//! паре, и с тем же seq тоже; версия книги не передаётся (решение 213: связи seq кадра с REST orderbook в документации
//! нет); тихая пара подтверждается по содержимому — стакан в памяти против ответа REST (book_print.rs, Тигран 08.11). Команды сторожа ядра: «переподписать пару» — {"op":"unsubscribe","args":[{instType, channel,
//! instId}]}, затем {"op":"subscribe",…} этой пары в этом же соединении (WebSocket intro: op «subscribe unsubscribe»,
//! пример unsubscribe), соседи не трогаются; каждая переподписка тратит 1 из «240 subscription requests/Hour/
//! connection» — venue.pair_resub_* задавать с этим запасом. «Переподключить соединение» — сессия заканчивается, ws.rs
//! подключается заново с теми же парами.

use std::collections::HashMap;
use std::sync::atomic::Ordering;

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::sync::mpsc::{self, error::TrySendError};
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::Message;

use crate::core::types::{Asset, OrderBook};
use crate::gateway::events::MarketEvent;
use crate::storage::journal::Level;

use super::book::{Alive, Books};
use super::ctx::{Ctx, ID};
use super::depth::{self, Msg};
use super::ws::{Cfg, End, invalidate};
use crate::gateway::feed_health::Cmd;
use crate::gateway::feed_reg::ConnHandle;

/// Учёт свежести соединения и канал команд сторожа (решение 202).
pub struct Watch<'a> {
    pub feed: &'a ConnHandle,
    pub cmd: &'a mut mpsc::UnboundedReceiver<Cmd>,
}

/// Наибольший размер одного запроса подписки, байт — «cannot exceed 4096 bytes at a time».
pub const MAX_SUB_BYTES: usize = 4096;

/// Что делает: сообщения подписки на канал стакана для символов — каждое не длиннее MAX_SUB_BYTES.
/// Принимает: канал (books5 …), символы. Возвращает: тексты сообщений по порядку. Ошибок нет.
pub fn sub_messages(channel: &str, symbols: &[&str]) -> Vec<String> {
    let mut out = Vec::new();
    let mut args: Vec<Value> = Vec::new();
    for s in symbols {
        let arg = json!({"instType": "SPOT", "channel": channel, "instId": s});
        args.push(arg);
        let text = json!({"op": "subscribe", "args": args}).to_string();
        if text.len() > MAX_SUB_BYTES && args.len() > 1 {
            let last = args.pop();
            out.push(json!({"op": "subscribe", "args": args}).to_string());
            args = last.into_iter().collect();
        }
    }
    if !args.is_empty() {
        out.push(json!({"op": "subscribe", "args": args}).to_string());
    }
    out
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
    let alive: Alive = Default::default();
    let by_symbol: HashMap<&str, &Asset> = assets.iter().map(|(a, s)| (s.as_str(), a)).collect();
    let syms: Vec<&str> = assets.iter().map(|(_, s)| s.as_str()).collect();
    let mut pending = sub_messages(&cfg.channel, &syms);
    pending.reverse();
    let mut last_seq: HashMap<String, i64> = HashMap::new();
    let mut sub_tick = tokio::time::interval(cfg.sub_gap);
    let mut ping = tokio::time::interval_at(Instant::now() + cfg.ping, cfg.ping);
    let mut last_rx = Instant::now();
    let mut dropped = 0u64;
    loop {
        tokio::select! {
            _ = sub_tick.tick(), if !pending.is_empty() => {
                let Some(m) = pending.pop() else { continue };
                if let Err(e) = sink.send(Message::Text(m.into())).await {
                    return End::Broken(format!("отправка подписки: {e}"));
                }
            }
            Some(c) = w.cmd.recv() => match c {
                Cmd::Reconnect => return End::Broken("сторож пар: переподключение соединения (решение 202)".into()),
                Cmd::Resub(ticker) => {
                    let Some((_, sym)) = assets.iter().find(|(a, _)| a.ticker == ticker) else { continue };
                    let arg = json!({"instType": "SPOT", "channel": cfg.channel, "instId": sym});
                    for op in ["unsubscribe", "subscribe"] {
                        let m = json!({"op": op, "args": [arg]}).to_string();
                        if let Err(e) = sink.send(Message::Text(m.into())).await {
                            return End::Broken(format!("отправка {op} (сторож пар): {e}"));
                        }
                    }
                    // Первый снимок после переподписки — ядру, даже с тем же seq (решение 109).
                    last_seq.remove(sym);
                    w.feed.resubscribed(&ticker);
                }
            },
            _ = ping.tick() => {
                if let Err(e) = sink.send(Message::Text("ping".into())).await {
                    return End::Broken(format!("отправка ping: {e}"));
                }
            }
            _ = tokio::time::sleep_until(last_rx + cfg.silence) => {
                return End::Broken(format!("нет кадров дольше venue.ws_silence_ms ({} мс)", cfg.silence.as_millis()));
            }
            next = stream.next() => {
                let frame = match next {
                    None => return End::Broken("соединение закрыто сервером".into()),
                    Some(Err(e)) => return End::Broken(format!("ошибка чтения: {e}")),
                    Some(Ok(f)) => f,
                };
                last_rx = Instant::now();
                alive.store(chrono::Utc::now().timestamp_millis(), Ordering::Relaxed);
                w.feed.alive();
                let text = match frame {
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
                let msg = depth::parse(&text);
                let is_book = matches!(&msg, Msg::Depth { .. } | Msg::Bad { symbol: Some(_), .. });
                if !is_book || matches!(ctx.flag("raw.book_frames"), Ok(true)) {
                    ctx.raw.write(ID, name, &text);
                }
                match msg {
                    Msg::Depth { symbol, seq, bids, asks, exchange_time } => {
                        let Some(asset) = by_symbol.get(symbol.as_str()) else { continue };
                        w.feed.frame(&asset.ticker);
                        if let Some(seq) = seq {
                            if last_seq.get(&symbol).is_some_and(|&s| seq <= s) {
                                books.touch(&asset.ticker);
                                continue;
                            }
                            last_seq.insert(symbol, seq);
                        }
                        let book = OrderBook {
                            venue: ctx.venue(),
                            asset: (*asset).clone(),
                            bids,
                            asks,
                            exchange_time,
                            received_at: chrono::Utc::now(),
                        };
                        books.put(book.clone(), alive.clone());
                        match out.try_send(MarketEvent::Book(book)) {
                            Ok(()) => {}
                            Err(TrySendError::Closed(_)) => return End::Done,
                            Err(TrySendError::Full(_)) => dropped += 1,
                        }
                    }
                    Msg::Bad { symbol, reason } => {
                        if let Some(a) = symbol.as_deref().and_then(|s| by_symbol.get(s)) {
                            invalidate(ctx, books, out, &a.ticker);
                        }
                        ctx.log(Level::Error, "venue.ws", format!("WebSocket {name}: кадр не разобран: {reason}")).await;
                    }
                    Msg::Error { inst, text } => {
                        if let Some(a) = inst.as_deref().and_then(|s| by_symbol.get(s)) {
                            invalidate(ctx, books, out, &a.ticker);
                        }
                        ctx.log(Level::Error, "venue.ws", format!("WebSocket {name}: ошибка Bitget: {text}")).await;
                    }
                    _ => {}
                }
                if dropped > 0 && dropped.is_power_of_two() {
                    tracing::warn!(conn = name, dropped, "ядро не успевает принимать стаканы Bitget");
                }
            }
        }
    }
}

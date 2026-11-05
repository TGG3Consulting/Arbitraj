//! Файл: venues/phemex/ws_conn.rs
//! Что делает: одна сессия публичного WebSocket стаканов Phemex: подключение, подписка orderbook.subscribe по одной паре
//! на запрос с паузой venue.ws_subs_per_sec, свой server.ping раз в venue.ws_ping_ms, тишина дольше venue.ws_silence_ms
//! — обрыв; кадры стакана → локальный стакан (local_book.rs) → память и ядру (без ожидания, try_send); разрыв номера —
//! пара недостоверна до следующего снимка; отказ подписки (error в ответе на её id) — пара недостоверна. Сырые кадры
//! стакана — в журнал только при raw.book_frames = да (решение 109).
//! Кто вызывает: ws.rs.
//! От чего зависит: depth.rs, local_book.rs, book.rs, symbols.rs, ws.rs (Cfg, End), ctx.rs, tokio-tungstenite.
//! Настройки: из ws.rs (Cfg); raw.book_frames [phemex].
//!
//! Официально (Spot Websocket API, https://phemex-docs.github.io/#subscribe-orderbook-2 и #heartbeat-2): подписка
//! {"id": <id>, "method": "orderbook.subscribe", "params": ["<symbol>"]}; heartbeat {"id": 0, "method":
//! "server.ping", "params": []} → {"error": null, "id": 0, "result": "pong"}. Подписка шлётся только при подключении
//! (урок 14): отписки одной пары нет — смена набора пар = новое соединение (market.rs).

use std::collections::HashMap;
use std::sync::atomic::Ordering;

use chrono::{TimeZone, Utc};
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use tokio::sync::mpsc::{self, error::TrySendError};
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::Message;

use crate::core::types::{Asset, OrderBook};
use crate::gateway::events::MarketEvent;
use crate::storage::journal::Level;

use super::book::{Alive, Books};
use super::ctx::{Ctx, ID};
use super::depth::{self, Msg};
use super::local_book::{Applied, LocalBook};
use super::symbols::Symbols;
use super::ws::{Cfg, End, invalidate};

/// Пара соединения: монета и масштабы цены и количества.
struct Pair<'a> {
    asset: &'a Asset,
    price_scale: u32,
    qty_scale: u32,
}

/// Сообщение подписки на стакан одной пары. Принимает: id запроса, символ. Ошибок нет.
pub fn sub_message(id: u64, symbol: &str) -> String {
    json!({"id": id, "method": "orderbook.subscribe", "params": [symbol]}).to_string()
}

/// Сессия: работает до обрыва. Возвращает причину окончания.
pub async fn session(
    ctx: &Ctx,
    name: &str,
    assets: &[(Asset, String)],
    symbols: &Symbols,
    books: &Books,
    out: &mpsc::Sender<MarketEvent>,
    cfg: Cfg,
) -> End {
    let rules = symbols.all();
    let mut pairs: HashMap<&str, Pair> = HashMap::new();
    for (a, s) in assets {
        match rules.get(&a.ticker) {
            Some(r) => {
                let p = Pair {
                    asset: a,
                    price_scale: r.price_scale,
                    qty_scale: r.base_scale,
                };
                pairs.insert(s.as_str(), p);
            }
            None => return End::Config(format!("{s}: нет правил пары в справочнике Phemex")),
        }
    }
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
    let alive: Alive = Default::default();
    // id запроса подписки → символ (ответ с ошибкой — пара недостоверна). id 0 — ping.
    let by_id: HashMap<u64, &str> = assets
        .iter()
        .enumerate()
        .map(|(i, (_, s))| (i as u64 + 1, s.as_str()))
        .collect();
    let mut pending: Vec<(u64, &str)> = by_id.iter().map(|(i, s)| (*i, *s)).collect();
    pending.sort_by_key(|(i, _)| std::cmp::Reverse(*i));
    let mut local: HashMap<String, LocalBook> = HashMap::new();
    let mut sub_tick = tokio::time::interval(cfg.sub_gap);
    let mut ping = tokio::time::interval_at(Instant::now() + cfg.ping, cfg.ping);
    let ping_msg = json!({"id": 0, "method": "server.ping", "params": []}).to_string();
    let mut last_rx = Instant::now();
    let mut dropped = 0u64;
    loop {
        tokio::select! {
            _ = sub_tick.tick(), if !pending.is_empty() => {
                let Some((id, s)) = pending.pop() else { continue };
                if let Err(e) = sink.send(Message::Text(sub_message(id, s).into())).await {
                    return End::Broken(format!("отправка подписки: {e}"));
                }
            }
            _ = ping.tick() => {
                if let Err(e) = sink.send(Message::Text(ping_msg.clone().into())).await {
                    return End::Broken(format!("отправка server.ping: {e}"));
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
                alive.store(Utc::now().timestamp_millis(), Ordering::Relaxed);
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
                let is_book = matches!(&msg, Msg::Book(_) | Msg::Bad { symbol: Some(_), .. });
                if !is_book || matches!(ctx.flag("raw.book_frames"), Ok(true)) {
                    ctx.raw.write(ID, name, &text);
                }
                let f = match msg {
                    Msg::Book(f) => f,
                    Msg::Reply { id, ok: false, text } => {
                        if let Some(p) = id.and_then(|i| by_id.get(&i)).and_then(|s| pairs.get(s)) {
                            invalidate(ctx, books, out, &p.asset.ticker);
                        }
                        ctx.log(Level::Error, "venue.ws", format!("WebSocket {name}: ошибка Phemex: {text}")).await;
                        continue;
                    }
                    Msg::Bad { symbol, reason } => {
                        if let Some(p) = symbol.as_deref().and_then(|s| pairs.get(s)) {
                            invalidate(ctx, books, out, &p.asset.ticker);
                        }
                        ctx.log(Level::Error, "venue.ws", format!("WebSocket {name}: кадр не разобран: {reason}")).await;
                        continue;
                    }
                    _ => continue,
                };
                let Some(p) = pairs.get(f.symbol.as_str()) else { continue };
                let lb = local.entry(f.symbol.clone()).or_default();
                match lb.apply(&f, cfg.strict) {
                    Applied::Changed => {}
                    Applied::Waiting => continue,
                    Applied::Gap(why) => {
                        invalidate(ctx, books, out, &p.asset.ticker);
                        let m = format!("WebSocket {name}: {} — {why}; стакан недостоверен до снимка (≤ 60 с)", f.symbol);
                        ctx.log(Level::Warn, "venue.ws", m).await;
                        continue;
                    }
                }
                let (bids, asks) = lb.top(cfg.levels, p.price_scale, p.qty_scale);
                let book = OrderBook {
                    venue: ctx.venue(),
                    asset: p.asset.clone(),
                    bids,
                    asks,
                    exchange_time: f.ts_ns.map(|ns| Utc.timestamp_nanos(ns)),
                    received_at: Utc::now(),
                };
                books.put(book.clone(), alive.clone());
                match out.try_send(MarketEvent::Book(book)) {
                    Ok(()) => {}
                    Err(TrySendError::Closed(_)) => return End::Done,
                    Err(TrySendError::Full(_)) => dropped += 1,
                }
                if dropped > 0 && dropped.is_power_of_two() {
                    tracing::warn!(conn = name, dropped, "ядро не успевает принимать стаканы Phemex");
                }
            }
        }
    }
}

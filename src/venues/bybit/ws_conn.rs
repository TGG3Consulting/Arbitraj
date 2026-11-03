//! Файл: venues/bybit/ws_conn.rs
//! Что делает: одна сессия публичного WebSocket спота Bybit: подключение, подписки пачками не больше 10 args
//! ({"op":"subscribe","req_id","args"}) с паузой, свой {"op":"ping"} раз в venue.ws_ping_ms, Pong на Ping-кадр,
//! локальные стаканы из снимка и изменений (local_book.rs) → память и ядру (try_send). Разрыв номера u — стакан пары
//! недостоверен и пересподписка этой пары (новый снимок); второй разрыв той же пары раньше venue.ws_reconnect_pause_ms —
//! переподключение соединения с паузой (урок 12: без бесконечного круга ресинков). Отказ подписки — стаканы её пачки
//! недостоверны.
//! Кто вызывает: ws.rs.
//! От чего зависит: depth.rs, local_book.rs, book.rs, ctx.rs, tokio-tungstenite.
//! Настройки: приходят из ws.rs (Cfg); raw.book_frames [bybit] — писать ли кадры стаканов в сырой журнал.
//!
//! Официально (Connect https://bybit-exchange.github.io/docs/v5/ws/connect): «Spot can input up to 10 args for each
//! subscription request»; ответ подписки спота — {"success", "ret_msg", "req_id", "op":"subscribe"}. Тишина считается от
//! последнего кадра (как решение 98): pong — тоже кадр. Кадры стаканов в сыром журнале — только при raw.book_frames
//! (решение 109: поток стаканов нагружает диск); ответы op пишутся всегда.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::Ordering;

use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TrySendError;
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::Message;

use crate::core::types::{Asset, OrderBook, VenueId};
use crate::gateway::events::MarketEvent;
use crate::gateway::tap::{TapData, TapStream, now_ns};
use crate::storage::journal::Level;

use super::book::{Alive, Books};
use super::ctx::{Ctx, ID};
use super::depth::{self, Msg, channel};
use super::local_book::{Applied, LocalBook};
use super::ws::{Cfg, End, invalidate};

/// Сколько args в одном запросе подписки спота — из официальной документации («up to 10 args for each subscription»).
pub const SUB_ARGS_MAX: usize = 10;

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
    let alive: Alive = Default::default();
    let by_symbol: HashMap<&str, &Asset> = assets.iter().map(|(a, s)| (s.as_str(), a)).collect();
    let topics: Vec<String> = assets.iter().map(|(_, s)| channel(cfg.levels, s)).collect();
    // Пачки подписки по SUB_ARGS_MAX; req_id → символы пачки (чтобы снять их стаканы при отказе).
    let mut pending: VecDeque<(String, Vec<String>)> = topics
        .chunks(SUB_ARGS_MAX)
        .enumerate()
        .map(|(i, c)| (format!("s{}", i + 1), c.to_vec()))
        .collect();
    let mut sent: HashMap<String, Vec<String>> = HashMap::new();
    let mut local: HashMap<String, LocalBook> = HashMap::new();
    let mut resynced: HashMap<String, Instant> = HashMap::new();
    let mut sub_tick = tokio::time::interval(cfg.sub_gap);
    let mut ping = tokio::time::interval(cfg.ping);
    let mut last_rx = Instant::now();
    let mut dropped = 0u64;
    let depth = cfg.levels as usize;
    let venue = VenueId(ID.to_string());
    loop {
        tokio::select! {
            _ = sub_tick.tick(), if !pending.is_empty() => {
                let Some((id, args)) = pending.pop_front() else { continue };
                let sub = json!({ "op": "subscribe", "req_id": id, "args": args });
                if let Err(e) = sink.send(Message::Text(sub.to_string().into())).await {
                    return End::Broken(format!("отправка подписки: {e}"));
                }
                sent.insert(id, args);
            }
            _ = ping.tick() => {
                let p = json!({ "op": "ping" });
                if let Err(e) = sink.send(Message::Text(p.to_string().into())).await {
                    return End::Broken(format!("отправка ping: {e}"));
                }
            }
            _ = tokio::time::sleep_until(last_rx + cfg.silence) => {
                return End::Broken(format!("нет кадров дольше {} мс", cfg.silence.as_millis()));
            }
            next = stream.next() => {
                let frame = match next {
                    None => return End::Broken("соединение закрыто сервером".into()),
                    Some(Err(e)) => return End::Broken(format!("ошибка чтения: {e}")),
                    Some(Ok(f)) => f,
                };
                let rx_ns = now_ns();
                last_rx = Instant::now();
                alive.store(chrono::Utc::now().timestamp_millis(), Ordering::Relaxed);
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
                match depth::parse(&t) {
                    Msg::Book { symbol, snapshot, u, ts, bids, asks } => {
                        let Some(asset) = by_symbol.get(symbol) else { continue };
                        if matches!(ctx.flag("raw.book_frames"), Ok(true)) {
                            ctx.raw.write(ID, name, &t);
                        }
                        ctx.taps.push(ID, &asset.ticker, TapStream::Book, name, rx_ns, || TapData::Text(t.to_string()));
                        // Без новой строки ключа на каждый кадр (урок 13): вставка — только у первого кадра пары.
                        if !local.contains_key(symbol) {
                            local.insert(symbol.to_string(), LocalBook::default());
                        }
                        let Some(lb) = local.get_mut(symbol) else { continue };
                        match lb.apply(snapshot, u, &bids, &asks) {
                            Applied::Changed => {
                                let (b, a) = lb.top(depth);
                                let book = OrderBook {
                                    venue: venue.clone(),
                                    asset: (*asset).clone(),
                                    bids: b,
                                    asks: a,
                                    exchange_time: ts.and_then(chrono::DateTime::from_timestamp_millis),
                                    received_at: chrono::Utc::now(),
                                };
                                books.put(book.clone(), alive.clone());
                                match out.try_send(MarketEvent::Book(book)) {
                                    Ok(()) => {}
                                    Err(TrySendError::Closed(_)) => return End::Done,
                                    Err(TrySendError::Full(_)) => dropped += 1,
                                }
                            }
                            Applied::Repeat => books.touch(&asset.ticker),
                            Applied::Gap(why) => {
                                invalidate(ctx, books, out, &asset.ticker);
                                let topic = channel(cfg.levels, symbol);
                                if resynced.get(symbol).is_some_and(|at| at.elapsed() < cfg.resync_gap) {
                                    return End::Broken(format!("{topic}: снова разрыв стакана ({why}) — переподключение"));
                                }
                                resynced.insert(symbol.to_string(), Instant::now());
                                log(ctx, Level::Warn, format!("WebSocket {name}: {topic}: {why} — пересподписка за новым снимком")).await;
                                for op in ["unsubscribe", "subscribe"] {
                                    let m = json!({ "op": op, "req_id": format!("r-{symbol}"), "args": [topic.clone()] });
                                    if let Err(e) = sink.send(Message::Text(m.to_string().into())).await {
                                        return End::Broken(format!("отправка {op}: {e}"));
                                    }
                                }
                            }
                        }
                    }
                    Msg::Ack { op, success, msg, req_id } => {
                        ctx.raw.write(ID, name, &t);
                        if op == "subscribe" && !success {
                            let batch = req_id.and_then(|id| sent.get(&id).cloned()).unwrap_or_else(|| topics.clone());
                            for (a, s) in assets {
                                if batch.contains(&channel(cfg.levels, s)) {
                                    invalidate(ctx, books, out, &a.ticker);
                                    local.remove(s);
                                }
                            }
                            log(ctx, Level::Error, format!("WebSocket {name}: отказ подписки ({}): {msg}", batch.join(", "))).await;
                        }
                    }
                    Msg::Bad(reason) => {
                        ctx.raw.write(ID, name, &t);
                        log(ctx, Level::Error, format!("WebSocket {name}: кадр не разобран: {reason}")).await;
                    }
                    Msg::Other => ctx.raw.write(ID, name, &t),
                }
                if dropped > 0 && pending.is_empty() && dropped.is_power_of_two() {
                    tracing::warn!(conn = name, dropped, "ядро не успевает принимать стаканы Bybit");
                }
            }
        }
    }
}

/// Запись в журнал.
async fn log(ctx: &Ctx, level: Level, msg: String) {
    let _ = ctx
        .journal
        .write(level, "venue.ws", Some(ID), msg, None)
        .await;
}

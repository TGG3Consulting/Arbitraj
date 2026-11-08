//! Файл: venues/kraken/ws_conn.rs
//! Что делает: одна сессия публичного WebSocket v2 стаканов Kraken: подключение, подписка всех пар соединения пачками
//! (venue.ws_streams_per_msg символов, не чаще venue.ws_subs_per_sec запросов в секунду), свой ping после venue.ws_ping_ms
//! без кадров, тишина дольше venue.ws_silence_ms — обрыв; кадры book — локальный стакан (local_book.rs) с проверкой
//! чек-суммы: не совпала — пара недостоверна и пересподписка этой пары (изменения до её нового снимка пропускаются);
//! второй разрыв той же пары раньше venue.ws_reconnect_pause_ms — переподключение (урок 12: без бесконечного круга
//! ресинков). Отказ подписки символа (success = false) — стакан пары недостоверен. Сырые кадры стакана — в журнал только
//! при raw.book_frames = да (решение 109).
//! Кто вызывает: ws.rs.
//! От чего зависит: depth.rs, local_book.rs, book.rs, ws.rs (Cfg, End), ws_req.rs, market.rs (Sub), ctx.rs,
//! gateway/feed_reg.rs (решение 202).
//! Настройки: из ws.rs (Cfg); raw.book_frames [kraken].
//!
//! Решение 202 (для всех бирж, Тигран 08.11): каждый кадр отмечается в учёте свежести (gateway/feed_reg.rs) — кадр
//! book по паре (снимок, изменение, кадр с разрывом чек-суммы, неразборчивый с символом); версия книги не передаётся
//! (номера у канала book нет — решение 213 по содержимому, book_print.rs). Команды сторожа ядра: «переподписать пару»
//! — unsubscribe, затем subscribe канала book этой пары в этом же соединении (ws_req.rs resub; book.md: «method:
//! unsubscribe», params channel, symbol, depth) — тот же путь, что пересподписка при разрыве чек-суммы: изменения до
//! нового снимка пропускаются, соседи не трогаются; лимита запросов подписки в документации нет (venue.ws_subs_per_sec
//! — только темп подписки при подключении). «Переподключить соединение» — сессия заканчивается, ws.rs подключается
//! заново с теми же парами.
//!
//! Официально: https://docs.kraken.com/exchange/api-reference/spot-websocket-v2/book.md (подписка, снимок, изменения,
//! «one acknowledgement per symbol»); https://docs.kraken.com/exchange/api-reference/spot-websocket.md (ping / pong).

use std::collections::{HashMap, HashSet};
use std::sync::atomic::Ordering;

use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc::{self, error::TrySendError};
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::Message;

use crate::core::types::OrderBook;
use crate::gateway::events::MarketEvent;
use crate::storage::journal::Level;

use super::book::{Alive, Books};
use super::ctx::{Ctx, ID};
use super::depth::{self, Msg};
use super::local_book::{Applied, LocalBook};
use super::market::Sub;
use super::ws::{Cfg, End, invalidate};
use super::ws_req::{book_requests, open, ping, resub};
use crate::gateway::feed_health::Cmd;
use crate::gateway::feed_reg::ConnHandle;

/// Учёт свежести соединения и канал команд сторожа (решение 202).
pub struct Watch<'a> {
    pub feed: &'a ConnHandle,
    pub cmd: &'a mut mpsc::UnboundedReceiver<Cmd>,
}

/// Сессия: работает до обрыва. Возвращает причину окончания.
pub async fn session(
    ctx: &Ctx,
    name: &str,
    subs: &[Sub],
    books: &Books,
    out: &mpsc::Sender<MarketEvent>,
    cfg: Cfg,
    w: Watch<'_>,
) -> End {
    let ws = match open(&cfg.url, cfg.connect_timeout).await {
        Ok(ws) => ws,
        Err(e) => return End::Broken(e),
    };
    let (mut sink, mut stream) = ws.split();
    w.feed.session_started();
    let by_ws: HashMap<&str, &Sub> = subs.iter().map(|s| (s.wsname.as_str(), s)).collect();
    let names: Vec<&str> = subs.iter().map(|s| s.wsname.as_str()).collect();
    let mut req_id = 1u64;
    for r in book_requests("subscribe", cfg.depth, &names, cfg.per_msg, req_id) {
        req_id += 1;
        if let Err(e) = sink.send(Message::Text(r.into())).await {
            return End::Broken(format!("отправка подписки: {e}"));
        }
        tokio::time::sleep(cfg.sub_gap).await;
    }
    let alive: Alive = Default::default();
    let mut local: HashMap<String, LocalBook> = HashMap::new();
    let mut last_gap: HashMap<String, Instant> = HashMap::new();
    // Пары после пересподписки: изменения до нового снимка (ещё летящие по старой подписке) пропускаются.
    let mut resync: HashSet<String> = HashSet::new();
    let (mut last_rx, mut pinged, mut dropped) = (Instant::now(), false, 0u64);
    loop {
        tokio::select! {
            _ = tokio::time::sleep_until(last_rx + cfg.ping), if !pinged => {
                pinged = true;
                req_id += 1;
                if let Err(e) = sink.send(Message::Text(ping(req_id).into())).await {
                    return End::Broken(format!("отправка ping: {e}"));
                }
            }
            Some(c) = w.cmd.recv() => match c {
                Cmd::Reconnect => return End::Broken("сторож пар: переподключение соединения (решение 202)".into()),
                Cmd::Resub(ticker) => {
                    let Some(s) = subs.iter().find(|s| s.asset.ticker == ticker) else { continue };
                    // Изменения до нового снимка — по старой подписке, пропускаются (как при разрыве).
                    resync.insert(s.wsname.clone());
                    if let Err(e) = resub(&mut sink, cfg.depth, &s.wsname, &mut req_id).await {
                        return End::Broken(format!("{e} (сторож пар)"));
                    }
                    w.feed.resubscribed(&ticker);
                }
            },
            _ = tokio::time::sleep_until(last_rx + cfg.silence) => {
                return End::Broken(format!("нет кадров дольше venue.ws_silence_ms ({} мс)", cfg.silence.as_millis()));
            }
            next = stream.next() => {
                let frame = match next {
                    None => return End::Broken("соединение закрыто сервером".into()),
                    Some(Err(e)) => return End::Broken(format!("ошибка чтения: {e}")),
                    Some(Ok(f)) => f,
                };
                (last_rx, pinged) = (Instant::now(), false);
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
                if !matches!(msg, Msg::Book(_)) || matches!(ctx.flag("raw.book_frames"), Ok(true)) {
                    ctx.raw.write(ID, name, &text);
                }
                let rows = match msg {
                    Msg::Book(rows) => rows,
                    Msg::Ack { success: false, symbol, error, method, .. } => {
                        if let Some(s) = symbol.as_deref().and_then(|s| by_ws.get(s)) {
                            invalidate(ctx, books, out, &s.asset.ticker);
                        }
                        let m = format!("WebSocket {name}: {method} {symbol:?} отклонён Kraken: {}", error.unwrap_or_default());
                        ctx.log(Level::Error, "venue.ws", m).await;
                        continue;
                    }
                    Msg::Bad(why) => {
                        ctx.log(Level::Error, "venue.ws", format!("WebSocket {name}: кадр не разобран: {why}")).await;
                        continue;
                    }
                    _ => continue,
                };
                for row in rows {
                    let d = match row {
                        Ok(d) => d,
                        Err((sym, why)) => {
                            if let Some(s) = sym.as_deref().and_then(|s| by_ws.get(s)) {
                                w.feed.frame(&s.asset.ticker);
                                invalidate(ctx, books, out, &s.asset.ticker);
                            }
                            ctx.log(Level::Error, "venue.ws", format!("WebSocket {name}: кадр book не разобран: {why}")).await;
                            continue;
                        }
                    };
                    let Some(sub) = by_ws.get(d.symbol.as_str()) else { continue };
                    w.feed.frame(&sub.asset.ticker);
                    if resync.contains(&d.symbol) {
                        if !d.snapshot {
                            continue;
                        }
                        resync.remove(&d.symbol);
                    }
                    let lb = local
                        .entry(d.symbol.clone())
                        .or_insert_with(|| LocalBook::new(cfg.depth as usize, sub.price_dp, sub.lot_dp));
                    if let Applied::Gap(why) = lb.apply(&d) {
                        invalidate(ctx, books, out, &sub.asset.ticker);
                        let again = last_gap.get(&d.symbol).is_some_and(|t| t.elapsed() < cfg.resync_gap);
                        if again {
                            return End::Broken(format!("{}: повторный разрыв стакана ({why})", d.symbol));
                        }
                        last_gap.insert(d.symbol.clone(), Instant::now());
                        resync.insert(d.symbol.clone());
                        ctx.log(Level::Warn, "venue.ws", format!("WebSocket {name}: {} — {why}; пересподписка пары", d.symbol)).await;
                        if let Err(e) = resub(&mut sink, cfg.depth, &d.symbol, &mut req_id).await {
                            return End::Broken(e);
                        }
                        w.feed.resubscribed(&sub.asset.ticker);
                        continue;
                    }
                    let (bids, asks) = lb.top(cfg.depth as usize);
                    let book = OrderBook {
                        venue: ctx.venue(),
                        asset: sub.asset.clone(),
                        bids,
                        asks,
                        exchange_time: d.exchange_time,
                        received_at: chrono::Utc::now(),
                    };
                    books.put(book.clone(), alive.clone());
                    match out.try_send(MarketEvent::Book(book)) {
                        Ok(()) => {}
                        Err(TrySendError::Closed(_)) => return End::Done,
                        Err(TrySendError::Full(_)) => dropped += 1,
                    }
                    if dropped > 0 && dropped.is_power_of_two() {
                        tracing::warn!(conn = name, dropped, "ядро не успевает принимать стаканы Kraken");
                    }
                }
            }
        }
    }
}

//! Файл: venues/gate/ws_conn.rs
//! Что делает: одна сессия WebSocket рынка Gate: подключение, подписки spot.order_book по одной с паузой
//! (venue.ws_subs_per_sec), свой spot.ping раз в venue.ws_ping_ms, Pong на Ping-кадр сервера, запись сырых кадров,
//! снимки стаканов → память и ядру; отказ подписки или неразборчивый стакан → стакан этой пары недостоверен;
//! spot.system upgrade → переподключение. Ядру стаканы отдаются без ожидания (try_send).
//! Кто вызывает: ws.rs.
//! От чего зависит: depth.rs, book.rs, ctx.rs, gateway/tap.rs, tokio-tungstenite.
//! Настройки: приходят из ws.rs (Cfg); raw.book_frames [gate] — писать ли кадры стаканов в сырой журнал.
//!
//! Решение 109 (как MEXC): снимок с тем же lastUpdateId, что прежний, ядру не отдаётся — стакан не менялся («Order book
//! ID, which is updated whenever the order book is changed», модель OrderBook REST; в канале — lastUpdateId); меньший
//! — устаревший, пропускается. Время кадра пары обновляется в обоих случаях (book.rs, touch; решение 118, А-🟠8).
//! Сравнение по номеру, а не по уровням: без копий уровней на каждый кадр (решения 130, 131).
//! Решение 125: каждый кадр стакана монеты с открытым окном — в приёмник окна как есть (текст).

use std::collections::HashMap;
use std::sync::atomic::Ordering;

use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TrySendError;
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::Message;

use crate::core::types::Asset;
use crate::gateway::events::MarketEvent;
use crate::gateway::tap::{TapData, TapStream, now_ns};
use crate::storage::journal::Level;

use super::book::{Alive, Books};
use super::ctx::{Ctx, ID};
use super::depth::{self, BOOK_CHANNEL, Msg};
use super::ws::{Cfg, End, invalidate};

/// Канал стакана для показа на странице окна: «spot.order_book CAKE_USDT 20 100ms». Ошибок нет.
pub fn channel(symbol: &str, levels: u64, interval: &str) -> String {
    format!("{BOOK_CHANNEL} {symbol} {levels} {interval}")
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
    let alive: Alive = Default::default();
    let by_symbol: HashMap<&str, &Asset> = assets.iter().map(|(a, s)| (s.as_str(), a)).collect();
    // Номер запроса подписки = номер пары в соединении (по нему ответ-отказ находит пару).
    let mut next_sub = 0usize;
    let mut sub_tick = tokio::time::interval(cfg.sub_gap);
    let mut ping = tokio::time::interval(cfg.ping);
    let mut last_rx = Instant::now();
    let mut dropped = 0u64;
    // Последний отданный ядру номер стакана по символу (решение 109).
    let mut last: HashMap<String, i64> = HashMap::new();
    loop {
        tokio::select! {
            _ = sub_tick.tick(), if next_sub < assets.len() => {
                let (_, sym) = &assets[next_sub];
                let p = json!([sym, cfg.levels.to_string(), cfg.interval]);
                let req = depth::request(BOOK_CHANNEL, "subscribe", p, next_sub as i64);
                next_sub += 1;
                if let Err(e) = sink.send(Message::Text(req.into())).await {
                    return End::Broken(format!("отправка подписки: {e}"));
                }
            }
            _ = ping.tick() => {
                if let Err(e) = sink.send(Message::Text(depth::ping().into())).await {
                    return End::Broken(format!("отправка spot.ping: {e}"));
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
                let m = depth::parse(&t);
                let is_book = matches!(m, Msg::Depth { .. } | Msg::Bad { .. });
                if !is_book || matches!(ctx.flag("raw.book_frames"), Ok(true)) {
                    ctx.raw.write(ID, name, &t);
                }
                match m {
                    Msg::Depth { symbol, update_id, bids, asks, exchange_time } => {
                        let Some(asset) = by_symbol.get(symbol.as_str()) else { continue };
                        ctx.taps.push(ID, &asset.ticker, TapStream::Book, name, rx_ns, || TapData::Text(t.to_string()));
                        if last.get(&symbol).is_some_and(|&id| update_id <= id) {
                            books.touch(&asset.ticker);
                            continue;
                        }
                        last.insert(symbol, update_id);
                        let book = depth::to_book((*asset).clone(), bids, asks, exchange_time);
                        books.put(book.clone(), alive.clone());
                        match out.try_send(MarketEvent::Book(book)) {
                            Ok(()) => {}
                            Err(TrySendError::Closed(_)) => return End::Done,
                            Err(TrySendError::Full(_)) => dropped += 1,
                        }
                    }
                    Msg::Bad { symbol, reason } => {
                        if let Some(s) = symbol.as_deref()
                            && let Some(a) = by_symbol.get(s)
                        {
                            invalidate(ctx, books, out, &a.ticker);
                            last.remove(s);
                        }
                        log(ctx, Level::Error, format!("WebSocket {name}: кадр не разобран: {reason}")).await;
                    }
                    Msg::Ack { id, error: Some(err) } => {
                        if let Some((a, s)) = id.and_then(|i| assets.get(i as usize)) {
                            invalidate(ctx, books, out, &a.ticker);
                            last.remove(s);
                        }
                        log(ctx, Level::Error, format!("WebSocket {name}: отказ подписки (id {id:?}): {err}")).await;
                    }
                    Msg::Upgrade => return End::Broken("spot.system: сервер обновляется (upgrade)".into()),
                    _ => {}
                }
                if dropped > 0 && dropped.is_power_of_two() {
                    tracing::warn!(conn = name, dropped, "ядро не успевает принимать стаканы Gate");
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

//! Файл: venues/mexc/ws_conn.rs
//! Что делает: одна сессия WebSocket рынка MEXC: подключение, подписки по одной с паузой ({"method":"SUBSCRIPTION"}),
//! свой {"method":"PING"} раз в venue.ws_ping_ms, ответ Pong на Ping-кадр сервера, запись сырых кадров, стаканы
//! (protobuf) → память и ядру, отказ подписки или неразборчивый стакан → стакан этой пары недостоверен.
//! Ядру стаканы отдаются без ожидания (try_send).
//! Кто вызывает: ws.rs.
//! От чего зависит: depth.rs, book.rs, ctx.rs, tokio-tungstenite.
//! Настройки: приходят из ws.rs (Cfg).
//!
//! Тишина считается от последнего кадра (как решение 98 у bsc_dex): любой кадр, включая PONG, — соединение живо.
//! Сырой журнал: текстовые кадры — как есть; двоичные (стаканы protobuf) — hex, только если raw.book_frames [mexc] = да
//! (решение 109: снимки стаканов MEXC идут потоком — запись каждого кадра нагружала диск и процессор).
//! Решение 109: снимок стакана, не отличающийся от прежнего (те же уровни), ядру не отдаётся — частичный стакан MEXC
//! присылается снимками и без изменений; свежесть — по соединению (alive) и по времени кадра пары (book.rs, touch —
//! и для пропущенного одинакового снимка; решение 118, А-🟠8).

use std::collections::{HashMap, VecDeque};
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

use crate::core::types::PriceLevel;

use super::book::{Alive, Books};
use super::ctx::{Ctx, ID};
use super::depth::{self, Msg};
use super::ws::{Cfg, End, invalidate};

/// Канал частичного стакана: «spot@public.limit.depth.v3.api.pb@<symbol>@<level>» (официальная документация).
pub fn channel(symbol: &str, levels: u64) -> String {
    format!("spot@public.limit.depth.v3.api.pb@{symbol}@{levels}")
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
    let by_channel: HashMap<String, String> = assets
        .iter()
        .map(|(a, s)| (channel(s, cfg.levels), a.ticker.clone()))
        .collect();
    let mut pending: VecDeque<String> =
        assets.iter().map(|(_, s)| channel(s, cfg.levels)).collect();
    let mut sub_tick = tokio::time::interval(cfg.sub_gap);
    let mut ping = tokio::time::interval(cfg.ping);
    let mut last_rx = Instant::now();
    let mut dropped = 0u64;
    // Последний отданный ядру снимок по символу (решение 109).
    let mut last: HashMap<String, (Vec<PriceLevel>, Vec<PriceLevel>)> = HashMap::new();
    loop {
        tokio::select! {
            _ = sub_tick.tick(), if !pending.is_empty() => {
                let Some(ch) = pending.pop_front() else { continue };
                let sub = json!({ "method": "SUBSCRIPTION", "params": [ch] });
                if let Err(e) = sink.send(Message::Text(sub.to_string().into())).await {
                    return End::Broken(format!("отправка подписки: {e}"));
                }
            }
            _ = ping.tick() => {
                let p = json!({ "method": "PING" });
                if let Err(e) = sink.send(Message::Text(p.to_string().into())).await {
                    return End::Broken(format!("отправка PING: {e}"));
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
                let msg = match frame {
                    Message::Binary(b) => {
                        if matches!(ctx.flag("raw.book_frames"), Ok(true)) {
                            ctx.raw.write(ID, name, &hex::encode(&b));
                        }
                        let m = depth::parse_binary(&b);
                        if let Msg::Depth { symbol, .. } | Msg::Bad { symbol: Some(symbol), .. } = &m
                            && let Some(a) = by_symbol.get(symbol.as_str())
                        {
                            ctx.taps.push(ID, &a.ticker, TapStream::Book, name, rx_ns, || TapData::Binary(b.to_vec()));
                        }
                        m
                    }
                    Message::Text(t) => {
                        ctx.raw.write(ID, name, &t);
                        depth::parse_text(&t)
                    }
                    Message::Ping(p) => {
                        if let Err(e) = sink.send(Message::Pong(p)).await {
                            return End::Broken(format!("отправка Pong: {e}"));
                        }
                        continue;
                    }
                    Message::Close(c) => return End::Broken(format!("сервер закрыл соединение: {c:?}")),
                    _ => continue,
                };
                match msg {
                    Msg::Depth { symbol, bids, asks, exchange_time } => {
                        let Some(asset) = by_symbol.get(symbol.as_str()) else { continue };
                        // Решение 109: тот же снимок — ядру не отдаём (стакан в памяти уже такой); время кадра пары
                        // обновляется (решение 118, А-🟠8).
                        if last.get(&symbol).is_some_and(|(b, a)| *b == bids && *a == asks) {
                            books.touch(&asset.ticker);
                            continue;
                        }
                        last.insert(symbol.clone(), (bids.clone(), asks.clone()));
                        let book = depth::to_book((*asset).clone(), bids, asks, exchange_time);
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
                            // Стакан снят — следующий снимок обязательно отдать ядру, даже если он такой же.
                            last.clear();
                        }
                        log(ctx, Level::Error, format!("WebSocket {name}: кадр не разобран: {reason}")).await;
                    }
                    // Ответ на подписку с кодом не 0 — отказ (формата ошибки в документации нет: пишем как есть).
                    Msg::Ack { code, msg } if code != 0 => {
                        for (ch, ticker) in &by_channel {
                            if msg.contains(ch.as_str()) {
                                invalidate(ctx, books, out, ticker);
                                last.clear();
                            }
                        }
                        log(ctx, Level::Error, format!("WebSocket {name}: отказ, код {code}: {msg}")).await;
                    }
                    _ => {}
                }
                if dropped > 0 && pending.is_empty() && dropped.is_power_of_two() {
                    tracing::warn!(conn = name, dropped, "ядро не успевает принимать стаканы MEXC");
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

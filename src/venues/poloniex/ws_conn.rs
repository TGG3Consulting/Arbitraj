//! Файл: venues/poloniex/ws_conn.rs
//! Что делает: одна сессия публичного WebSocket Poloniex: подключение, подписка на канал book по одному символу в
//! сообщении с паузой venue.ws_subs_per_sec, свой ping раз в venue.ws_ping_ms, тишина дольше venue.ws_silence_ms —
//! обрыв; снимки стакана → память и ядру (без ожидания, try_send); неразборчивый снимок пары → стакан пары
//! недостоверен. Сырые кадры стакана — в журнал только при raw.book_frames = да (решение 109).
//! Кто вызывает: ws.rs.
//! От чего зависит: depth.rs, book.rs, ctx.rs, tokio-tungstenite.
//! Настройки: приходят из ws.rs (Cfg); raw.book_frames [poloniex].
//!
//! Решение 109: снимок с тем же или меньшим id (SeqId), что прежний, ядру не отдаётся — стакан не менялся (канал book
//! шлёт снимки каждые 100 мс — «The order book updates every 100ms»); время кадра пары обновляется (book.rs, touch).
//! Сравнение по номеру, без копий уровней (решения 130, 131). Нет id — снимок отдаётся всегда.
//! Официально (Spot → WebSocket, Market Data → Book): подписка {"event": "subscribe", "channel": ["book"], "symbols":
//! [...], "depth": N}. Сколько символов в одном сообщении и подписок на соединение, в документации нет — по одному
//! символу в сообщении (урок 12); подписка шлётся только при подключении (урок 14).

use std::collections::HashMap;
use std::sync::atomic::Ordering;

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
use super::depth::{self, BOOK, Msg};
use super::ws::{Cfg, End, invalidate};

/// Что делает: сообщение подписки на канал book для одного символа. Ошибок нет.
pub fn sub_message(symbol: &str, depth: u64) -> String {
    json!({"event": "subscribe", "channel": [BOOK], "symbols": [symbol], "depth": depth})
        .to_string()
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
    let mut pending: Vec<String> = assets
        .iter()
        .rev()
        .map(|(_, s)| sub_message(s, cfg.depth))
        .collect();
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
            _ = ping.tick() => {
                if let Err(e) = sink.send(Message::Text(depth::ping().into())).await {
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
                let is_book = matches!(&msg, Msg::Books(_) | Msg::Bad { symbol: Some(_), .. });
                if !is_book || matches!(ctx.flag("raw.book_frames"), Ok(true)) {
                    ctx.raw.write(ID, name, &text);
                }
                match msg {
                    Msg::Books(list) => {
                        for d in list {
                            let Some(asset) = by_symbol.get(d.symbol.as_str()) else { continue };
                            if let Some(seq) = d.seq {
                                if last_seq.get(&d.symbol).is_some_and(|&s| seq <= s) {
                                    books.touch(&asset.ticker);
                                    continue;
                                }
                                last_seq.insert(d.symbol, seq);
                            }
                            let book = OrderBook {
                                venue: ctx.venue(),
                                asset: (*asset).clone(),
                                bids: d.bids,
                                asks: d.asks,
                                exchange_time: d.exchange_time,
                                received_at: chrono::Utc::now(),
                            };
                            books.put(book.clone(), alive.clone());
                            match out.try_send(MarketEvent::Book(book)) {
                                Ok(()) => {}
                                Err(TrySendError::Closed(_)) => return End::Done,
                                Err(TrySendError::Full(_)) => dropped += 1,
                            }
                        }
                    }
                    Msg::Bad { symbol, reason } => {
                        if let Some(a) = symbol.as_deref().and_then(|s| by_symbol.get(s)) {
                            invalidate(ctx, books, out, &a.ticker);
                        }
                        ctx.log(Level::Error, "venue.ws", format!("WebSocket {name}: кадр не разобран: {reason}")).await;
                    }
                    // В ошибке нет символа — какая подписка не прошла, неизвестно. Переподключение повторило бы ту же
                    // ошибку по кругу (урок 12): запись в журнал; пара без снимков сама остаётся без котировки (book.rs).
                    Msg::Error(text) => {
                        ctx.log(Level::Error, "venue.ws", format!("WebSocket {name}: ошибка Poloniex: {text}")).await;
                    }
                    _ => {}
                }
                if dropped > 0 && dropped.is_power_of_two() {
                    tracing::warn!(conn = name, dropped, "ядро не успевает принимать стаканы Poloniex");
                }
            }
        }
    }
}

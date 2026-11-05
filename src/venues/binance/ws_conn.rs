//! Файл: venues/binance/ws_conn.rs
//! Что делает: одна сессия WebSocket рынка Binance (комбинированный поток): подключение, SUBSCRIBE пачками по
//! venue.ws_streams_per_msg потоков с паузой, Pong на Ping сервера (с тем же payload), стаканы → память и ядру;
//! отказ подписки или неразборчивый стакан → стакан пары недостоверен. Ядру — без ожидания (try_send).
//! Кто вызывает: ws.rs.
//! От чего зависит: depth.rs, book.rs, ctx.rs, gateway/tap.rs, tokio-tungstenite.
//! Настройки: из ws.rs (Cfg); raw.book_frames [binance] — писать ли кадры стаканов в сырой журнал.
//!
//! Решение 109: снимок с тем же lastUpdateId ядру не отдаётся и в Decimal не разбирается — частичный стакан приходит
//! по таймеру; свежесть — по соединению (alive) и по кадру пары (book.rs, touch). Тишина — от последнего кадра.
//! Официально (web-socket-streams): {"method":"SUBSCRIBE","params":[…],"id":1} → {"result":null,"id":1}; ping сервера
//! раз в 20 с — «respond with a pong frame, copying the ping's payload».

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
use super::depth::{self, Msg};
use super::ws::{Cfg, End, invalidate};

/// Имя потока частичного стакана: «<symbol>@depth<N>[@100ms]», символ строчными (официально). Ошибок нет.
pub fn stream(symbol: &str, levels: u64, suffix: &str) -> String {
    format!("{}@depth{levels}{suffix}", symbol.to_lowercase())
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
    let by_symbol: HashMap<String, &Asset> =
        assets.iter().map(|(a, s)| (s.to_lowercase(), a)).collect();
    let names: Vec<String> = assets
        .iter()
        .map(|(_, s)| self::stream(s, cfg.levels, cfg.suffix))
        .collect();
    let mut batches: Vec<Vec<String>> = names.chunks(cfg.per_msg).map(<[_]>::to_vec).collect();
    batches.reverse();
    // id запроса → его потоки (отказ подписки снимает стаканы этих пар).
    let mut sent: HashMap<String, Vec<String>> = HashMap::new();
    let mut next_id = 0u64;
    let mut sub_tick = tokio::time::interval(cfg.sub_gap);
    let mut last_rx = Instant::now();
    let mut dropped = 0u64;
    // Последний отданный ядру lastUpdateId по символу (решение 109).
    let mut last: HashMap<String, i64> = HashMap::new();
    loop {
        tokio::select! {
            _ = sub_tick.tick(), if !batches.is_empty() => {
                let Some(params) = batches.pop() else { continue };
                next_id += 1;
                let sub = json!({ "method": "SUBSCRIBE", "params": params, "id": next_id });
                if let Err(e) = sink.send(Message::Text(sub.to_string().into())).await {
                    return End::Broken(format!("отправка подписки: {e}"));
                }
                sent.insert(next_id.to_string(), params);
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
                    Msg::Depth { symbol, book } => {
                        if matches!(ctx.flag("raw.book_frames"), Ok(true)) {
                            ctx.raw.write(ID, name, &t);
                        }
                        let Some(asset) = by_symbol.get(&symbol) else { continue };
                        ctx.taps.push(ID, &asset.ticker, TapStream::Book, name, rx_ns, || TapData::Text(t.to_string()));
                        if last.get(&symbol) == Some(&book.last_update_id) {
                            books.touch(&asset.ticker);
                            continue;
                        }
                        let (bids, asks) = match depth::levels(&book) {
                            Ok(l) => l,
                            Err(why) => {
                                invalidate(ctx, books, out, &asset.ticker);
                                last.remove(&symbol);
                                log(ctx, Level::Error, format!("WebSocket {name}: стакан {symbol} не разобран: {why}")).await;
                                continue;
                            }
                        };
                        last.insert(symbol, book.last_update_id);
                        let ob = depth::to_book((*asset).clone(), bids, asks);
                        books.put(ob.clone(), alive.clone());
                        match out.try_send(MarketEvent::Book(ob)) {
                            Ok(()) => {}
                            Err(TrySendError::Closed(_)) => return End::Done,
                            Err(TrySendError::Full(_)) => dropped += 1,
                        }
                    }
                    Msg::Error { code, msg, id } => {
                        ctx.raw.write(ID, name, &t);
                        for s in sent.get(&id).into_iter().flatten() {
                            let sym = s.split('@').next().unwrap_or_default();
                            if let Some(a) = by_symbol.get(sym) {
                                invalidate(ctx, books, out, &a.ticker);
                                last.remove(sym);
                            }
                        }
                        log(ctx, Level::Error, format!("WebSocket {name}: отказ запроса id {id}, код {code}: {msg}")).await;
                    }
                    Msg::Bad(why) => {
                        ctx.raw.write(ID, name, &t);
                        log(ctx, Level::Error, format!("WebSocket {name}: кадр не разобран: {why}")).await;
                    }
                    Msg::Ack(_) | Msg::Other => ctx.raw.write(ID, name, &t),
                }
                if dropped > 0 && batches.is_empty() && dropped.is_power_of_two() {
                    tracing::warn!(conn = name, dropped, "ядро не успевает принимать стаканы Binance");
                }
            }
        }
    }
}

/// Запись в журнал. Ошибок нет.
async fn log(ctx: &Ctx, level: Level, msg: String) {
    ctx.log(level, "venue.ws", msg).await;
}

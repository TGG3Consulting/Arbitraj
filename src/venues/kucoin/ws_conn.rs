//! Файл: venues/kucoin/ws_conn.rs
//! Что делает: одна сессия WebSocket рынка KuCoin: подключение по токену, ожидание welcome, подписки темами по ≤ 100
//! символов с паузой, свой ping раз в pingInterval токена, тишина дольше pingInterval + pingTimeout — обрыв; снимки
//! стакана → память и ядру (без ожидания, try_send); отказ подписки (type error) или неразборчивый снимок → стакан
//! пары недостоверен. Сырые кадры стакана — в журнал только при raw.book_frames = да (решение 109), в окно — всегда.
//! Кто вызывает: ws.rs; open() — feed.rs, user_stream_conn.rs.
//! От чего зависит: depth.rs, book.rs, bullet.rs, ctx.rs, gateway/feed_reg.rs, tokio-tungstenite.
//! Настройки: приходят из ws.rs (Cfg); raw.book_frames [kucoin].
//!
//! Официально (Classic WebSocket introduction): «The connection is usable only after the welcome message arrives»;
//! подписка {"id","type":"subscribe","topic","privateChannel","response"}, несколько символов в теме через запятую;
//! ping — «at least once every pingInterval». Тема снимка — «/spotMarket/level2Depth{5|50}:{symbol},{symbol}», «a topic
//! supports up to 100 symbols» (спецификация SDK spec/ws/openapi-spot-public.json, x-topic). Живая запись подписки и
//! кадров: tests/data/cex_samples_2026_11_04/kucoin_ws.jsonl.
//! Решение 202 (для всех бирж, 08.11): каждый кадр отмечается в учёте свежести (gateway/feed_reg.rs) — кадр стакана по
//! паре (и неразборчивый тоже); номера в кадре Level5/Level50 нет — подтверждение тихой пары по содержимому
//! (book_print.rs). Команды сторожа ядра: «переподписать пару» — {"type":"unsubscribe"} и {"type":"subscribe"} ТОЙ ЖЕ
//! темы, в которой пара подписана (до SYMBOLS_PER_TOPIC пар), в этом же соединении; отписка одной пары из темы с
//! несколькими символами в прочитанной документации (спецификация SDK, запись 04.11) не описана — переподписывается вся
//! тема (вопрос — docs/TASKS.md), соседи по теме тоже получают начало прогрева. Отказ подписки (type error с id темы) —
//! стаканы темы недостоверны, как при подключении. «Переподключить соединение» — сессия заканчивается, ws.rs
//! подключается заново с теми же парами (новый токен).

use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use tokio::net::TcpStream;
use tokio::sync::mpsc::{self, error::TrySendError};
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

use crate::core::types::{Asset, OrderBook};
use crate::gateway::events::MarketEvent;
use crate::gateway::tap::{TapData, TapStream, now_ns};
use crate::storage::journal::Level;

use super::book::{Alive, Books};
use super::bullet::Token;
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

/// Символов в одной теме — официально «a topic supports up to 100 symbols».
pub const SYMBOLS_PER_TOPIC: usize = 100;
/// Тем на одно соединение Spot — официально «Spot/Margin is limited to 400».
pub const TOPICS_PER_CONN: usize = 400;

/// Открытое соединение.
pub type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// Тема снимка стакана для символов. Ошибок нет.
pub fn topic(levels: u64, symbols: &[&str]) -> String {
    format!("/spotMarket/level2Depth{levels}:{}", symbols.join(","))
}

/// Что делает: подключение по токену и ожидание welcome. Принимает: токен, срок. Ошибка — текст (без токена).
pub async fn open(t: &Token, timeout: Duration) -> Result<Ws, String> {
    let connect =
        tokio::time::timeout(timeout, tokio_tungstenite::connect_async(t.url.as_str())).await;
    let mut ws = match connect {
        Err(_) => return Err("не удалось подключиться за отведённое время".into()),
        Ok(Err(e)) => {
            return Err(format!(
                "подключение: {}",
                e.to_string().replace(t.url.as_str(), "***")
            ));
        }
        Ok(Ok((ws, _))) => ws,
    };
    let welcome = tokio::time::timeout(timeout, async {
        while let Some(Ok(f)) = ws.next().await {
            if let Message::Text(s) = f
                && matches!(depth::parse(&s), Msg::Welcome)
            {
                return true;
            }
        }
        false
    });
    match welcome.await {
        Ok(true) => Ok(ws),
        _ => Err("нет welcome за http.timeout_ms — соединение не готово".into()),
    }
}

/// Сессия: работает до обрыва. Возвращает причину окончания.
#[allow(clippy::too_many_arguments)]
pub async fn session(
    ctx: &Ctx,
    name: &str,
    assets: &[(Asset, String)],
    books: &Books,
    out: &mpsc::Sender<MarketEvent>,
    cfg: Cfg,
    token: Token,
    w: Watch<'_>,
) -> End {
    let ws = match open(&token, cfg.connect_timeout).await {
        Ok(ws) => ws,
        Err(e) => return End::Broken(e),
    };
    let (mut sink, mut stream) = ws.split();
    w.feed.session_started();
    let alive: Alive = Default::default();
    let by_symbol: HashMap<&str, &Asset> = assets.iter().map(|(a, s)| (s.as_str(), a)).collect();
    let syms: Vec<&str> = assets.iter().map(|(_, s)| s.as_str()).collect();
    let mut pending: Vec<(String, Vec<&str>)> = syms
        .chunks(SYMBOLS_PER_TOPIC)
        .enumerate()
        .map(|(i, c)| (format!("sub{i}"), c.to_vec()))
        .collect();
    let by_id: HashMap<String, Vec<&str>> = pending.iter().cloned().collect();
    pending.reverse();
    let mut sub_tick = tokio::time::interval(cfg.sub_gap);
    let mut ping = tokio::time::interval_at(Instant::now() + token.ping, token.ping);
    let silence = token.ping + token.ping_timeout;
    let mut last_rx = Instant::now();
    let mut dropped = 0u64;
    loop {
        tokio::select! {
            _ = sub_tick.tick(), if !pending.is_empty() => {
                let Some((id, chunk)) = pending.pop() else { continue };
                let sub = json!({"id": id, "type": "subscribe", "topic": topic(cfg.levels, &chunk), "response": true});
                if let Err(e) = sink.send(Message::Text(sub.to_string().into())).await {
                    return End::Broken(format!("отправка подписки: {e}"));
                }
            }
            Some(c) = w.cmd.recv() => match c {
                Cmd::Reconnect => return End::Broken("сторож пар: переподключение соединения (решение 202)".into()),
                Cmd::Resub(ticker) => {
                    let Some((_, sym)) = assets.iter().find(|(a, _)| a.ticker == ticker) else { continue };
                    let Some((id, chunk)) = by_id.iter().find(|(_, c)| c.contains(&sym.as_str())) else { continue };
                    let t = topic(cfg.levels, chunk);
                    // Отказ подписки — тем же id темы (стаканы темы недостоверны), отписки — своим id.
                    for (op, rid) in [("unsubscribe", format!("{id}-un")), ("subscribe", id.clone())] {
                        let m = json!({"id": rid, "type": op, "topic": t, "response": true});
                        if let Err(e) = sink.send(Message::Text(m.to_string().into())).await {
                            return End::Broken(format!("отправка {op} (сторож пар): {e}"));
                        }
                    }
                    for s in chunk {
                        if let Some(a) = by_symbol.get(s) {
                            w.feed.resubscribed(&a.ticker);
                        }
                    }
                }
            },
            _ = ping.tick() => {
                let p = json!({"id": now_ns().to_string(), "type": "ping"});
                if let Err(e) = sink.send(Message::Text(p.to_string().into())).await {
                    return End::Broken(format!("отправка ping: {e}"));
                }
            }
            _ = tokio::time::sleep_until(last_rx + silence) => {
                return End::Broken(format!("нет кадров дольше pingInterval + pingTimeout ({} мс)", silence.as_millis()));
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
                    Msg::Depth { symbol, bids, asks, exchange_time } => {
                        let Some(asset) = by_symbol.get(symbol.as_str()) else { continue };
                        w.feed.frame(&asset.ticker);
                        ctx.taps.push(ID, &asset.ticker, TapStream::Book, name, rx_ns, || TapData::Text(text.to_string()));
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
                            w.feed.frame(&a.ticker);
                            invalidate(ctx, books, out, &a.ticker);
                        }
                        ctx.log(Level::Error, "venue.ws", format!("WebSocket {name}: кадр не разобран: {reason}")).await;
                    }
                    Msg::Error { id, text } => {
                        for s in id.as_ref().and_then(|i| by_id.get(i)).into_iter().flatten() {
                            if let Some(a) = by_symbol.get(s) {
                                invalidate(ctx, books, out, &a.ticker);
                            }
                        }
                        ctx.log(Level::Error, "venue.ws", format!("WebSocket {name}: ошибка KuCoin: {text}")).await;
                    }
                    _ => {}
                }
                if dropped > 0 && dropped.is_power_of_two() {
                    tracing::warn!(conn = name, dropped, "ядро не успевает принимать стаканы KuCoin");
                }
            }
        }
    }
}

//! Файл: venues/htx/ws_conn.rs
//! Что делает: одна сессия WebSocket рынка HTX: подключение, подписки по одной с паузой venue.ws_subs_per_sec, ответ
//! {"pong": n} на каждый {"ping": n} сервера, тишина дольше venue.ws_silence_ms — обрыв; двоичные кадры распаковываются
//! (GZIP), снимки стакана → память и ядру (без ожидания, try_send); ошибка по паре или неразборчивый снимок → стакан
//! пары недостоверен. Сырые кадры стакана — в журнал только при raw.book_frames = да (решение 109).
//! Кто вызывает: ws.rs.
//! От чего зависит: depth.rs, book.rs, ctx.rs, tokio-tungstenite, gateway/feed_reg.rs (решение 202).
//! Настройки: приходят из ws.rs (Cfg); raw.book_frames [htx].
//!
//! Решение 202 (для всех бирж, Тигран 08.11): каждый кадр отмечается в учёте свежести (gateway/feed_reg.rs) — снимок
//! mbp.refresh по паре (и с прежним seqNum, и неразборчивый с символом); seqNum не передаётся (решение 213: version
//! ответа GET /market/depth — «Internal data», связь с seqNum кадра в документации не описана — подтверждение тихой
//! пары по содержимому, book_print.rs). Команды сторожа ядра: «переподписать пару» — {"unsub": тема пары, "id": символ}
//! («Websocket Market Data → Unsubscribe»: ответ status ok и unsubbed), затем подписка той же темы; оба сообщения — в
//! очередь отправки с темпом venue.ws_subs_per_sec, соседи не трогаются; прежний seqNum пары забывается — первый
//! снимок новой подписки отдаётся ядру. Ошибка на id отписки (символ) — как ошибка подписки: стакан пары недостоверен
//! до нового снимка. «Переподключить соединение» — сессия заканчивается, ws.rs подключается заново с теми же парами.
//!
//! Решение 109: снимок с тем же или меньшим seqNum, что прежний, ядру не отдаётся — стакан не менялся; время кадра пары
//! обновляется (book.rs, touch). Сравнение по номеру, без копий уровней (решения 130, 131). Нет seqNum — снимок всегда.
//! Официально («Websocket Market Data»): подписка {"sub": "market.<символ>.mbp.refresh.<уровни>", "id": "<свой id>"} —
//! свой id — символ (по нему видно, у какой пары ошибка подписки); подписка шлётся только при подключении (урок 14).

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
use super::depth::{self, Msg};
use super::ws::{Cfg, End, invalidate};
use crate::gateway::feed_health::Cmd;
use crate::gateway::feed_reg::ConnHandle;

/// Учёт свежести соединения и канал команд сторожа (решение 202).
pub struct Watch<'a> {
    pub feed: &'a ConnHandle,
    pub cmd: &'a mut mpsc::UnboundedReceiver<Cmd>,
}

/// Сообщение подписки на снимки стакана символа. Ошибок нет.
pub fn sub_message(symbol: &str, levels: u64) -> String {
    json!({"sub": format!("market.{symbol}.mbp.refresh.{levels}"), "id": symbol}).to_string()
}

/// Сообщение отписки от снимков стакана символа («Websocket Market Data → Unsubscribe»: {"unsub", "id"}). Ошибок нет.
pub fn unsub_message(symbol: &str, levels: u64) -> String {
    json!({"unsub": format!("market.{symbol}.mbp.refresh.{levels}"), "id": symbol}).to_string()
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
    let mut pending: Vec<String> = assets
        .iter()
        .rev()
        .map(|(_, s)| sub_message(s, cfg.levels))
        .collect();
    let mut last_seq: HashMap<String, i64> = HashMap::new();
    let mut sub_tick = tokio::time::interval(cfg.sub_gap);
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
                    let Some((_, s)) = assets.iter().find(|(a, _)| a.ticker == ticker) else { continue };
                    // Очередь отправки берёт с конца: сначала уйдёт отписка, затем подписка.
                    pending.push(sub_message(s, cfg.levels));
                    pending.push(unsub_message(s, cfg.levels));
                    last_seq.remove(s);
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
                last_rx = Instant::now();
                alive.store(chrono::Utc::now().timestamp_millis(), Ordering::Relaxed);
                w.feed.alive();
                let text = match frame {
                    Message::Binary(b) => match depth::gunzip(&b) {
                        Ok(t) => t,
                        Err(e) => {
                            ctx.log(Level::Error, "venue.ws", format!("WebSocket {name}: {e}")).await;
                            continue;
                        }
                    },
                    Message::Text(t) => t.to_string(),
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
                    Msg::Ping(n) => {
                        let pong = json!({ "pong": n }).to_string();
                        if let Err(e) = sink.send(Message::Text(pong.into())).await {
                            return End::Broken(format!("отправка pong: {e}"));
                        }
                    }
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
                            w.feed.frame(&a.ticker);
                            invalidate(ctx, books, out, &a.ticker);
                        }
                        ctx.log(Level::Error, "venue.ws", format!("WebSocket {name}: кадр не разобран: {reason}")).await;
                    }
                    Msg::Error { id, text } => {
                        if let Some(a) = id.as_deref().and_then(|s| by_symbol.get(s)) {
                            invalidate(ctx, books, out, &a.ticker);
                        }
                        ctx.log(Level::Error, "venue.ws", format!("WebSocket {name}: ошибка HTX: {text}")).await;
                    }
                    _ => {}
                }
                if dropped > 0 && dropped.is_power_of_two() {
                    tracing::warn!(conn = name, dropped, "ядро не успевает принимать стаканы HTX");
                }
            }
        }
    }
}

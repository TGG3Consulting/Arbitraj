//! Файл: venues/coinex/ws_conn.rs
//! Что делает: одна сессия WebSocket рынка CoinEx: подключение, ОДИН запрос depth.subscribe со всеми рынками соединения
//! (if_full = true), свой server.ping раз в venue.ws_ping_ms, тишина дольше venue.ws_silence_ms — обрыв; двоичные кадры —
//! распаковка (inflate.rs, буфер соединения), снимки стакана → память и ядру (без ожидания, try_send); ошибка
//! подписки — обрыв с причиной; неполный пуш или неразборчивый снимок → стакан пары недостоверен.
//! Сырые кадры стакана — в журнал только при raw.book_frames = да (решение 109).
//! Кто вызывает: ws.rs.
//! От чего зависит: depth.rs, inflate.rs, book.rs, ctx.rs, gateway/feed_reg.rs, tokio-tungstenite.
//! Настройки: приходят из ws.rs (Cfg); raw.book_frames [coinex].
//!
//! Решение 202 (для всех бирж, Тигран 08.11): каждый кадр отмечается в учёте свежести (gateway/feed_reg.rs) — пуш стакана
//! по паре (в том числе с тем же updated_at — полный пуш «every 1 minute» без изменений книги, неполный и неразборчивый
//! с рынком). Команды сторожа ядра: «переподписать пару» — {"method":"depth.unsubscribe","params":{"market_list":[]}}
//! («Empty list to unsubscribe to all markets»; пример «All markets» в WS Market Depth), затем depth.subscribe со ВСЕМИ
//! рынками соединения, как при подключении. Почему не одна пара: отписка одного рынка в документации есть, а что делает
//! повторный depth.subscribe (добавляет рынки или заменяет прежний список) — нет; «отписать все + подписать все» даёт
//! один и тот же итог при любом из двух вариантов. Соседи по соединению получают новый полный пуш (if_full = true —
//! локального стакана нет) и начинают прогрев. Отказ одного из двух запросов (ответ с ошибкой на его id) — обрыв с
//! причиной, как отказ подписки при подключении. «Переподключить соединение» — сессия заканчивается, ws.rs подключается
//! заново с теми же парами.
//!
//! Решение 109: снимок с тем же updated_at, что прежний, ядру не отдаётся (стакан не менялся); время кадра пары
//! обновляется (book.rs, touch). Номеров последовательности у CoinEx нет (WS Market Depth).
//! Один запрос на соединение, а не по запросу на рынок: что делает повторный depth.subscribe (добавляет рынки или
//! заменяет прежние), документация не говорит — проверить на живом подключении.

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
use super::inflate::Inflater;
use super::ws::{Cfg, End, invalidate};
pub use super::ws_msg::sub_message;
use super::ws_msg::{SUB_ID, sub_message_id, unsub_all_message};
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
    if let Err(e) = sink
        .send(Message::Text(sub_message(&syms, cfg.levels).into()))
        .await
    {
        return End::Broken(format!("отправка подписки: {e}"));
    }
    let mut zip = Inflater::default();
    let mut told_format = false;
    let mut last_at: HashMap<String, i64> = HashMap::new();
    let mut ping = tokio::time::interval_at(Instant::now() + cfg.ping, cfg.ping);
    let mut ping_id = SUB_ID;
    let mut last_rx = Instant::now();
    let mut dropped = 0u64;
    // id запросов переподписки сторожем (решение 202): ответ с ошибкой на них — обрыв.
    let mut resub_ids: Vec<i64> = Vec::new();
    loop {
        tokio::select! {
            _ = ping.tick() => {
                ping_id += 1;
                let p = json!({"method": "server.ping", "params": {}, "id": ping_id}).to_string();
                if let Err(e) = sink.send(Message::Text(p.into())).await {
                    return End::Broken(format!("отправка server.ping: {e}"));
                }
            }
            Some(c) = w.cmd.recv() => match c {
                Cmd::Reconnect => return End::Broken("сторож пар: переподключение соединения (решение 202)".into()),
                Cmd::Resub(ticker) => {
                    if !assets.iter().any(|(a, _)| a.ticker == ticker) { continue }
                    let (un, sub) = (ping_id + 1, ping_id + 2);
                    ping_id = sub;
                    resub_ids.extend([un, sub]);
                    for m in [unsub_all_message(un), sub_message_id(&syms, cfg.levels, sub)] {
                        if let Err(e) = sink.send(Message::Text(m.into())).await {
                            return End::Broken(format!("отправка переподписки (сторож пар): {e}"));
                        }
                    }
                    // Новый полный пуш каждой пары — в память и ядру, даже с прежним updated_at.
                    last_at.clear();
                    for (a, _) in assets {
                        w.feed.resubscribed(&a.ticker);
                    }
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
                let text: &str = match &frame {
                    Message::Text(t) => t.as_str(),
                    // Распаковка — в буфер соединения, текст берётся из него без копии (урок 13).
                    Message::Binary(b) => match zip.text(b, cfg.max_frame) {
                        Ok((t, f)) => {
                            if !told_format {
                                told_format = true;
                                ctx.log(Level::Info, "venue.ws", format!("WebSocket {name}: кадры сжаты {}", f.name())).await;
                            }
                            t
                        }
                        Err(why) => return End::Broken(why),
                    },
                    Message::Ping(p) => {
                        if let Err(e) = sink.send(Message::Pong(p.clone())).await {
                            return End::Broken(format!("отправка Pong: {e}"));
                        }
                        continue;
                    }
                    Message::Close(c) => return End::Broken(format!("сервер закрыл соединение: {c:?}")),
                    _ => continue,
                };
                let msg = depth::parse(text);
                let is_book = matches!(&msg, Msg::Depth { .. } | Msg::Bad { symbol: Some(_), .. });
                if !is_book || matches!(ctx.flag("raw.book_frames"), Ok(true)) {
                    ctx.raw.write(ID, name, text);
                }
                match msg {
                    Msg::Depth { symbol, full, bids, asks, updated_at } => {
                        let Some(asset) = by_symbol.get(symbol.as_str()) else { continue };
                        w.feed.frame(&asset.ticker);
                        if !full {
                            invalidate(ctx, books, out, &asset.ticker);
                            ctx.log(Level::Error, "venue.ws", format!("WebSocket {name}: {symbol} — неполный пуш (is_full = false) при подписке if_full = true; стакан недостоверен")).await;
                            continue;
                        }
                        if let Some(at) = updated_at {
                            if last_at.get(&symbol).is_some_and(|&s| s == at) {
                                books.touch(&asset.ticker);
                                continue;
                            }
                            last_at.insert(symbol, at);
                        }
                        let book = OrderBook {
                            venue: ctx.venue(),
                            asset: (*asset).clone(),
                            bids,
                            asks,
                            exchange_time: updated_at.and_then(chrono::DateTime::from_timestamp_millis),
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
                    Msg::Error { id: Some(SUB_ID), code, text } => {
                        return End::Broken(format!("подписка depth.subscribe отклонена, код {code}: {text}"));
                    }
                    Msg::Error { id: Some(id), code, text } if resub_ids.contains(&id) => {
                        return End::Broken(format!("переподписка сторожем отклонена, код {code}: {text}"));
                    }
                    Msg::Error { code, text, .. } => {
                        ctx.log(Level::Error, "venue.ws", format!("WebSocket {name}: ошибка CoinEx, код {code}: {text}")).await;
                    }
                    _ => {}
                }
                if dropped > 0 && dropped.is_power_of_two() {
                    tracing::warn!(conn = name, dropped, "ядро не успевает принимать стаканы CoinEx");
                }
            }
        }
    }
}

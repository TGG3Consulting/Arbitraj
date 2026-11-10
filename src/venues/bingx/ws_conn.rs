//! Файл: venues/bingx/ws_conn.rs
//! Что делает: одна сессия WebSocket BingX: подключение, подписки по одной с паузой,
//! ответ «Pong» на ping сразу, запись сырых сообщений, стаканы → память и ядру,
//! отказ подписки или неразборчивый стакан → стакан этой пары недостоверен. Монета пишется окном (решение 125) —
//! текст её кадра стакана (после GZIP) со временем получения уходит в приёмник окна (ctx.taps), ядро не ждёт.
//! Ядру стаканы отдаются без ожидания (try_send): медленное ядро не задерживает ответ на ping.
//! Кто вызывает: ws.rs.
//! От чего зависит: depth.rs, book.rs, ctx.rs, gateway/feed_reg.rs, tokio-tungstenite.
//! Настройки: приходят из ws.rs (Cfg).
//!
//! Решение 202 (для всех бирж, 08.11): каждое сообщение отмечается в учёте свежести (gateway/feed_reg.rs) — стакан по
//! паре (и неразборчивый тоже); data.lastUpdateId кадра не передаётся (решение 213: в ответе REST стакана номера нет —
//! подтверждение тихой пары по содержимому, book_print.rs). Команды сторожа ядра: «переподписать пару» —
//! {"id","reqType":"unsub","dataType":"<symbol>@depth<N>"}, затем {"id","reqType":"sub",…} в этом же соединении (docs-v3,
//! Spot → Websocket Market Data: reqType «sub / unsub»; тот же вид отписки — feed.rs), соседи не трогаются; отказ этой
//! подписки (код ≠ 0) — стакан пары недостоверен, как при подключении. «Переподключить соединение» — сессия
//! заканчивается, ws.rs подключается заново с теми же парами.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::Ordering;

use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TrySendError;
use tokio_tungstenite::tungstenite::Message;

use crate::core::types::Asset;
use crate::gateway::events::MarketEvent;
use crate::gateway::tap::{TapData, TapStream, now_ns};
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
    // t79: пары соединения подписаны в этой сессии — стакан из ответа REST сторожа (book_rest.rs) только с её
    // живостью.
    books.bind(assets, &alive);
    let by_symbol: HashMap<&str, &Asset> = assets.iter().map(|(a, s)| (s.as_str(), a)).collect();
    let mut pending: VecDeque<(String, String)> = assets
        .iter()
        .map(|(a, s)| (a.ticker.clone(), format!("{s}@depth{}", cfg.levels)))
        .collect();
    let mut sub_ids: HashMap<String, String> = HashMap::new();
    let mut sub_tick = tokio::time::interval(cfg.sub_gap);
    let mut dropped = 0u64;
    loop {
        tokio::select! {
            _ = sub_tick.tick(), if !pending.is_empty() => {
                let Some((ticker, data_type)) = pending.pop_front() else { continue };
                let id = format!("{name}-{}", sub_ids.len() + 1);
                sub_ids.insert(id.clone(), ticker);
                let sub = json!({ "id": id, "reqType": "sub", "dataType": data_type });
                if let Err(e) = sink.send(Message::Text(sub.to_string().into())).await {
                    return End::Broken(format!("отправка подписки: {e}"));
                }
            }
            Some(c) = w.cmd.recv() => match c {
                Cmd::Reconnect => return End::Broken("сторож пар: переподключение соединения (решение 202)".into()),
                Cmd::Resub(ticker) => {
                    let Some((_, sym)) = assets.iter().find(|(a, _)| a.ticker == ticker) else { continue };
                    let data_type = format!("{sym}@depth{}", cfg.levels);
                    let n = sub_ids.len() + 1;
                    // Отказ подписки — по id из sub_ids (стакан пары недостоверен); отписка — своим id.
                    sub_ids.insert(format!("{name}-{n}"), ticker.clone());
                    for (req, id) in [("unsub", format!("{name}-{n}-un")), ("sub", format!("{name}-{n}"))] {
                        let m = json!({ "id": id, "reqType": req, "dataType": data_type });
                        if let Err(e) = sink.send(Message::Text(m.to_string().into())).await {
                            return End::Broken(format!("отправка {req} (сторож пар): {e}"));
                        }
                    }
                    w.feed.resubscribed(&ticker);
                }
            },
            next = tokio::time::timeout(cfg.silence, stream.next()) => {
                let frame = match next {
                    Err(_) => return End::Broken(format!("нет сообщений дольше {} мс", cfg.silence.as_millis())),
                    Ok(None) => return End::Broken("соединение закрыто сервером".into()),
                    Ok(Some(Err(e))) => return End::Broken(format!("ошибка чтения: {e}")),
                    Ok(Some(Ok(f))) => f,
                };
                let rx_ns = now_ns();
                // Любое сообщение (стакан, ping, ответ) — соединение живо (см. book.rs).
                alive.store(chrono::Utc::now().timestamp_millis(), Ordering::Relaxed);
                w.feed.alive();
                let text = match frame {
                    Message::Binary(b) => match depth::gunzip(&b) {
                        Ok(t) => t,
                        Err(e) => return End::Broken(e),
                    },
                    Message::Text(t) => t.to_string(),
                    Message::Close(c) => return End::Broken(format!("сервер закрыл соединение: {c:?}")),
                    _ => continue,
                };
                ctx.raw.write(ID, name, &text);
                match depth::parse(&text) {
                    Msg::Ping => {
                        if let Err(e) = sink.send(Message::Text("Pong".into())).await {
                            return End::Broken(format!("отправка Pong: {e}"));
                        }
                    }
                    Msg::Depth { symbol, bids, asks, exchange_time } => {
                        let Some(asset) = by_symbol.get(symbol.as_str()) else { continue };
                        w.feed.frame(&asset.ticker);
                        ctx.taps.push(ID, &asset.ticker, TapStream::Book, name, rx_ns, || TapData::Text(text.clone()));
                        let book = depth::to_book((*asset).clone(), bids, asks, exchange_time);
                        books.put(book.clone(), alive.clone());
                        match out.try_send(MarketEvent::Book(book)) {
                            Ok(()) => {}
                            Err(TrySendError::Closed(_)) => return End::Done,
                            // Ядро не успевает: событие пропущено, свежий стакан уже в памяти для котировок.
                            Err(TrySendError::Full(_)) => dropped += 1,
                        }
                    }
                    Msg::BadDepth { symbol, reason } => {
                        if let Some(a) = by_symbol.get(symbol.as_str()) {
                            w.feed.frame(&a.ticker);
                            ctx.taps.push(ID, &a.ticker, TapStream::Book, name, rx_ns, || TapData::Text(text.clone()));
                            invalidate(ctx, books, out, &a.ticker);
                        }
                        log(ctx, Level::Error, format!("WebSocket {name}: стакан {symbol} не разобран: {reason}"), &text).await;
                    }
                    Msg::SubAck { id, code, msg } if code != 0 => {
                        if let Some(ticker) = sub_ids.get(&id) {
                            invalidate(ctx, books, out, ticker);
                        }
                        log(ctx, Level::Error, format!("WebSocket {name}: подписка {id} отклонена, код {code}: {msg}"), &text).await;
                    }
                    _ => {}
                }
                if dropped > 0 && pending.is_empty() && dropped.is_power_of_two() {
                    tracing::warn!(conn = name, dropped, "ядро не успевает принимать стаканы");
                }
            }
        }
    }
}

/// Запись в журнал с сырым сообщением.
async fn log(ctx: &Ctx, level: Level, msg: String, raw: &str) {
    let _ = ctx
        .journal
        .write(
            level,
            "venue.ws",
            Some(ID),
            msg,
            Some(json!({ "raw": raw })),
        )
        .await;
}

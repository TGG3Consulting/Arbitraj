//! Файл: venues/bingx/ws_conn.rs
//! Что делает: одна сессия WebSocket BingX: подключение, подписки по одной с паузой,
//! ответ «Pong» на ping сразу, запись сырых сообщений, стаканы → память и ядру,
//! отказ подписки или неразборчивый стакан → стакан этой пары недостоверен.
//! Ядру стаканы отдаются без ожидания (try_send): медленное ядро не задерживает ответ на ping.
//! Кто вызывает: ws.rs.
//! От чего зависит: depth.rs, book.rs, ctx.rs, tokio-tungstenite.
//! Настройки: приходят из ws.rs (Cfg).

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::Ordering;

use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TrySendError;
use tokio_tungstenite::tungstenite::Message;

use crate::core::types::Asset;
use crate::gateway::events::MarketEvent;
use crate::storage::journal::Level;

use super::book::{Alive, Books};
use super::ctx::{Ctx, ID};
use super::depth::{self, Msg};
use super::ws::{Cfg, End, invalidate};

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
            next = tokio::time::timeout(cfg.silence, stream.next()) => {
                let frame = match next {
                    Err(_) => return End::Broken(format!("нет сообщений дольше {} мс", cfg.silence.as_millis())),
                    Ok(None) => return End::Broken("соединение закрыто сервером".into()),
                    Ok(Some(Err(e))) => return End::Broken(format!("ошибка чтения: {e}")),
                    Ok(Some(Ok(f))) => f,
                };
                // Любое сообщение (стакан, ping, ответ) — соединение живо (см. book.rs).
                alive.store(chrono::Utc::now().timestamp_millis(), Ordering::Relaxed);
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

//! Файл: venues/bingx/ws.rs
//! Что делает: одно соединение WebSocket с рыночными данными BingX: подключение, подписки
//! с ограничением частоты, ответ «Pong» на каждый ping, запись сырых сообщений, стаканы → ядру,
//! обнаружение обрыва по тишине, переподключение. При обрыве все стаканы соединения
//! объявляются недостоверными (торговли по ним нет, пока не придут свежие).
//! Кто вызывает: market.rs (по задаче на соединение).
//! От чего зависит: ctx.rs, depth.rs, book.rs, gateway/events, tokio-tungstenite.
//! Настройки: venue.ws_url, venue.ws_subs_per_sec, venue.ws_silence_ms, venue.ws_reconnect_pause_ms,
//! venue.depth_levels, http.timeout_ms (время на подключение).
//!
//! Официально: адрес спота wss://open-api-ws.bingx.com/market; все сообщения сжаты GZIP;
//! подписка {"id","reqType":"sub","dataType":"<symbol>@depth<N>"}; подписки не чаще 10/с (FAQ);
//! без ответа на ping сервер разрывает соединение (FAQ-05).

use std::collections::{HashMap, VecDeque};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

use crate::core::types::Asset;
use crate::gateway::events::MarketEvent;
use crate::storage::journal::Level;

use super::book::Books;
use super::ctx::Ctx;
use super::depth::{self, Msg};

/// Почему сессия закончилась.
enum End {
    /// Канал ядра закрыт — работа окончена.
    Done,
    /// Обрыв — переподключиться.
    Broken(String),
    /// Нужная настройка не задана — соединение останавливается (без бесконечных повторов).
    Config(String),
}

/// Работает, пока жив канал out: подключается заново после каждого обрыва.
/// Принимает: окружение, имя соединения, монеты (монета, символ BingX), стаканы, канал ядра.
pub async fn run(
    ctx: Ctx,
    name: String,
    assets: Vec<(Asset, String)>,
    books: Books,
    out: mpsc::Sender<MarketEvent>,
) {
    while !out.is_closed() {
        let end = session(&ctx, &name, &assets, &books, &out).await;
        if matches!(end, End::Done) {
            return;
        }
        for (a, _) in &assets {
            books.drop_ticker(&a.ticker);
            let _ = out
                .send(MarketEvent::Invalidated {
                    venue: ctx.venue(),
                    ticker: a.ticker.clone(),
                })
                .await;
        }
        let pause = match (end, ctx.int("venue.ws_reconnect_pause_ms")) {
            (End::Broken(reason), Ok(pause)) => {
                let msg = format!("WebSocket {name}: {reason}; переподключение");
                let _ = ctx
                    .journal
                    .write(Level::Warn, "venue.ws", Some(super::ctx::ID), msg, None)
                    .await;
                pause
            }
            (End::Config(reason), _)
            | (
                End::Broken(_),
                Err(crate::gateway::errors::GatewayError {
                    message: reason, ..
                }),
            ) => {
                let msg = format!("WebSocket {name} остановлен: {reason}");
                let _ = ctx
                    .journal
                    .write(Level::Error, "venue.ws", Some(super::ctx::ID), msg, None)
                    .await;
                return;
            }
            (End::Done, _) => return,
        };
        tokio::time::sleep(Duration::from_millis(pause)).await;
    }
}

/// Одна сессия соединения. Возвращает причину окончания.
async fn session(
    ctx: &Ctx,
    name: &str,
    assets: &[(Asset, String)],
    books: &Books,
    out: &mpsc::Sender<MarketEvent>,
) -> End {
    let cfg = match config(ctx) {
        Ok(c) => c,
        Err(e) => return End::Config(e),
    };
    match stream_loop(ctx, name, assets, books, out, cfg).await {
        Ok(()) => End::Done,
        Err(e) => End::Broken(e),
    }
}

/// Настройки соединения.
struct Cfg {
    url: String,
    levels: u64,
    per_sec: u64,
    silence: Duration,
    connect_timeout: Duration,
}

/// Читает настройки соединения. Ошибка — текст «не задана …».
fn config(ctx: &Ctx) -> Result<Cfg, String> {
    Ok(Cfg {
        url: ctx.text("venue.ws_url").map_err(|e| e.message)?,
        levels: ctx.int("venue.depth_levels").map_err(|e| e.message)?,
        per_sec: ctx.int("venue.ws_subs_per_sec").map_err(|e| e.message)?,
        silence: Duration::from_millis(ctx.int("venue.ws_silence_ms").map_err(|e| e.message)?),
        connect_timeout: ctx.http_timeout().map_err(|e| e.message)?,
    })
}

/// Подключение и чтение. Ok — канал ядра закрыт; Err — причина обрыва.
async fn stream_loop(
    ctx: &Ctx,
    name: &str,
    assets: &[(Asset, String)],
    books: &Books,
    out: &mpsc::Sender<MarketEvent>,
    cfg: Cfg,
) -> Result<(), String> {
    let Cfg {
        url,
        levels,
        per_sec,
        silence,
        connect_timeout,
    } = cfg;
    let alive: super::book::Alive = Default::default();
    let (ws, _) = tokio::time::timeout(
        connect_timeout,
        tokio_tungstenite::connect_async(url.as_str()),
    )
    .await
    .map_err(|_| "не удалось подключиться за отведённое время".to_string())?
    .map_err(|e| format!("подключение: {e}"))?;
    let (mut sink, mut stream) = ws.split();
    let by_symbol: HashMap<&str, &Asset> = assets.iter().map(|(a, s)| (s.as_str(), a)).collect();
    let mut pending: VecDeque<String> = assets
        .iter()
        .map(|(_, s)| format!("{s}@depth{levels}"))
        .collect();
    let mut sub_tick = tokio::time::interval(Duration::from_millis(1000 / per_sec.max(1)));
    let mut sent = 0u64;
    loop {
        tokio::select! {
            _ = sub_tick.tick(), if !pending.is_empty() => {
                let Some(dt) = pending.pop_front() else { continue };
                sent += 1;
                let sub = json!({ "id": format!("{name}-{sent}"), "reqType": "sub", "dataType": dt });
                sink.send(Message::Text(sub.to_string().into())).await.map_err(|e| format!("отправка подписки: {e}"))?;
            }
            next = tokio::time::timeout(silence, stream.next()) => {
                let frame = match next {
                    Err(_) => return Err(format!("нет сообщений дольше {} мс", silence.as_millis())),
                    Ok(None) => return Err("соединение закрыто сервером".into()),
                    Ok(Some(Err(e))) => return Err(format!("ошибка чтения: {e}")),
                    Ok(Some(Ok(f))) => f,
                };
                // Любое сообщение (стакан, ping, ответ) — соединение живо (см. book.rs).
                alive.store(chrono::Utc::now().timestamp_millis(), std::sync::atomic::Ordering::Relaxed);
                let text = match frame {
                    Message::Binary(b) => depth::gunzip(&b)?,
                    Message::Text(t) => t.to_string(),
                    Message::Close(c) => return Err(format!("сервер закрыл соединение: {c:?}")),
                    _ => continue,
                };
                ctx.raw.write(super::ctx::ID, name, &text);
                match depth::parse(&text) {
                    Ok(Msg::Ping) => sink.send(Message::Text("Pong".into())).await.map_err(|e| format!("отправка Pong: {e}"))?,
                    Ok(Msg::Depth { symbol, bids, asks, exchange_time }) => {
                        let Some(asset) = by_symbol.get(symbol.as_str()) else { continue };
                        let book = depth::to_book((*asset).clone(), bids, asks, exchange_time);
                        books.put(book.clone(), alive.clone());
                        if out.send(MarketEvent::Book(book)).await.is_err() {
                            return Ok(());
                        }
                    }
                    Ok(Msg::SubAck { code, msg }) if code != 0 => {
                        let m = format!("WebSocket {name}: подписка отклонена, код {code}: {msg}");
                        let _ = ctx.journal.write(Level::Error, "venue.ws", Some(super::ctx::ID), m, Some(json!({ "raw": text }))).await;
                    }
                    Ok(_) => {}
                    Err(e) => tracing::warn!(conn = name, %e, "непонятное сообщение стакана"),
                }
            }
        }
    }
}

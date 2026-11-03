//! Файл: venues/bybit/feed.rs
//! Что делает: соединение окна (решение 125) для одной монеты и одного канала на время окна — лента сделок
//! publicTrade.{symbol} или стакан всей записываемой глубины orderbook.{venue.window_depth_limit}.{symbol} (снимок и
//! изменения): подключение к venue.ws_url, {"op":"subscribe"}, свой {"op":"ping"}, Pong на Ping-кадр, каждый кадр данных
//! (текст как есть) — в приёмник окна (ctx.taps) со временем получения; обрыв — журнал, пауза venue.ws_reconnect_pause_ms,
//! переподключение (после него Bybit пришлёт новый снимок); окно закрыто — {"op":"unsubscribe"} и закрытие.
//! Кто вызывает: tap.rs (запуск при открытии окна).
//! От чего зависит: ws.rs (Cfg, config), depth.rs (ack), ctx.rs, gateway/tap.rs.
//! Настройки: venue.ws_url, venue.ws_silence_ms, venue.ws_ping_ms, venue.ws_reconnect_pause_ms, http.timeout_ms.
//!
//! Официально: Trade https://bybit-exchange.github.io/docs/v5/websocket/public/trade — «publicTrade.{symbol}», «a single
//! message may have up to 1024 trades»; Orderbook — «orderbook.{depth}.{symbol}»; отписка — {"op":"unsubscribe",
//! "args":[…]} (Connect https://bybit-exchange.github.io/docs/v5/ws/connect).

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use tokio::sync::watch;
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::Message;

use crate::gateway::tap::{TapData, TapStream, now_ns};
use crate::storage::journal::Level;

use super::ctx::{Ctx, ID};
use super::ws::{Cfg, config};

enum End {
    Stop,
    Broken(String),
}

/// Работает, пока окно открыто. Принимает: окружение, тикер, канал, поток окна, выключатель. Ошибок нет.
pub async fn run(
    ctx: Ctx,
    ticker: String,
    ch: String,
    kind: TapStream,
    mut stop: watch::Receiver<bool>,
) {
    let name = format!("window {ch}");
    while !*stop.borrow() {
        let end = match config(&ctx) {
            Ok(cfg) => session(&ctx, &name, &ticker, &ch, kind, &mut stop, cfg).await,
            Err(e) => {
                log(
                    &ctx,
                    Level::Error,
                    format!("канал {ch} для окна не пишется: {e}"),
                )
                .await;
                return;
            }
        };
        let End::Broken(r) = end else { return };
        let Ok(pause) = ctx.int("venue.ws_reconnect_pause_ms") else {
            return;
        };
        log(
            &ctx,
            Level::Warn,
            format!("WebSocket {name}: {r}; переподключение (в записи окна — пропуск)"),
        )
        .await;
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_millis(pause)) => {}
            _ = stop.changed() => return,
        }
    }
}

/// Одна сессия канала. Возвращает причину окончания.
async fn session(
    ctx: &Ctx,
    name: &str,
    ticker: &str,
    ch: &str,
    kind: TapStream,
    stop: &mut watch::Receiver<bool>,
    cfg: Cfg,
) -> End {
    let connect = tokio::time::timeout(
        cfg.connect_timeout,
        tokio_tungstenite::connect_async(cfg.url.as_str()),
    );
    let ws = tokio::select! {
        r = connect => match r {
            Err(_) => return End::Broken("не удалось подключиться за отведённое время".into()),
            Ok(Err(e)) => return End::Broken(format!("подключение: {e}")),
            Ok(Ok((ws, _))) => ws,
        },
        _ = stop.changed() => return End::Stop,
    };
    let (mut sink, mut stream) = ws.split();
    let sub = json!({ "op": "subscribe", "args": [ch] });
    if let Err(e) = sink.send(Message::Text(sub.to_string().into())).await {
        return End::Broken(format!("отправка подписки: {e}"));
    }
    let mut ping = tokio::time::interval(cfg.ping);
    let mut last_rx = Instant::now();
    loop {
        tokio::select! {
            _ = stop.changed() => {
                let unsub = json!({ "op": "unsubscribe", "args": [ch] });
                let _ = sink.send(Message::Text(unsub.to_string().into())).await;
                let _ = sink.close().await;
                return End::Stop;
            }
            _ = ping.tick() => {
                if let Err(e) = sink.send(Message::Text(json!({ "op": "ping" }).to_string().into())).await {
                    return End::Broken(format!("отправка ping: {e}"));
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
                match frame {
                    Message::Text(t) => match super::depth::ack(&t) {
                        Some(a) if a.op.as_deref() == Some("subscribe") && a.success == Some(false) => {
                            log(ctx, Level::Error, format!("WebSocket {name}: отказ подписки {ch}: {}", a.ret_msg.unwrap_or_default())).await;
                        }
                        Some(_) => {}
                        // В соединении одна подписка — каждый кадр данных относится к этому каналу монеты.
                        None => ctx.taps.push(ID, ticker, kind, name, rx_ns, || TapData::Text(t.to_string())),
                    },
                    Message::Ping(p) => {
                        if let Err(e) = sink.send(Message::Pong(p)).await {
                            return End::Broken(format!("отправка Pong: {e}"));
                        }
                    }
                    Message::Close(c) => return End::Broken(format!("сервер закрыл соединение: {c:?}")),
                    _ => {}
                }
            }
        }
    }
}

/// Запись в журнал. Ошибок нет.
async fn log(ctx: &Ctx, level: Level, msg: String) {
    let _ = ctx
        .journal
        .write(level, "venue.ws", Some(ID), msg, None)
        .await;
}

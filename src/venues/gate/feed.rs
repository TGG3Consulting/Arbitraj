//! Файл: venues/gate/feed.rs
//! Что делает: соединение окна (решение 125) для одной монеты и одного канала на время окна — лента сделок
//! (spot.trades) или изменения стакана (spot.order_book_update): подключение к venue.ws_url, подписка, свой spot.ping
//! раз в venue.ws_ping_ms, Pong на Ping-кадр, каждый кадр данных — в приёмник окна (ctx.taps) как есть, со временем
//! получения; обрыв — журнал, пауза venue.ws_reconnect_pause_ms, переподключение; окно закрыто — отписка (unsubscribe)
//! и закрытие. У изменений стакана — ещё снимок REST (tap_snap.rs) по первому кадру и при разрыве номеров.
//! Кто вызывает: tap.rs (запуск при открытии окна).
//! От чего зависит: ws.rs (Cfg, config), depth.rs (запросы, ответы), tap_snap.rs, ctx.rs, gateway/tap.rs.
//! Настройки: через ws::config — venue.ws_url, venue.ws_silence_ms, venue.ws_ping_ms, venue.ws_reconnect_pause_ms,
//! http.timeout_ms; снимок — venue.window_depth_limit (tap_snap.rs).
//!
//! Официально (Spot WebSocket v4 https://www.gate.com/docs/developers/apiv4/ws/en/): spot.trades — payload [пары],
//! «Only the taker side is notified», одна сделка в уведомлении (id, create_time_ms, side, amount, price); spot.
//! order_book_update — payload [пара, интервал], интервал 20ms (глубина 20) или 100ms (глубина 100).

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::sync::watch;
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::Message;

use crate::gateway::tap::{TapData, TapStream, now_ns};
use crate::storage::journal::Level;

use super::ctx::{Ctx, ID};
use super::depth::{self, Msg};
use super::tap_snap::Versions;
use super::ws::{Cfg, config};

/// Допустимые интервалы spot.order_book_update — из официальной документации.
pub const DIFF_INTERVALS: [&str; 2] = ["20ms", "100ms"];

/// Канал окна.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Feed {
    /// Лента сделок spot.trades.
    Trades,
    /// Изменения стакана spot.order_book_update (интервал — venue.window_diff_interval).
    Diff,
}

impl Feed {
    /// Имя канала. Ошибок нет.
    pub fn name(self) -> &'static str {
        match self {
            Feed::Trades => "spot.trades",
            Feed::Diff => "spot.order_book_update",
        }
    }

    /// payload подписки по документации. Принимает: символ, интервал (только у Diff). Ошибок нет.
    pub fn payload(self, symbol: &str, interval: &str) -> Value {
        match self {
            Feed::Trades => json!([symbol]),
            Feed::Diff => json!([symbol, interval]),
        }
    }
}

enum End {
    Stop,
    Broken(String),
}

/// Работает, пока окно открыто. Принимает: окружение, тикер, символ, вид, интервал, выключатель окна. Ошибок нет.
pub async fn run(
    ctx: Ctx,
    ticker: String,
    symbol: String,
    feed: Feed,
    interval: String,
    mut stop: watch::Receiver<bool>,
) {
    let name = format!("window {} {symbol}", feed.name());
    while !*stop.borrow() {
        let end = match config(&ctx) {
            Ok(cfg) => {
                session(
                    &ctx,
                    &name,
                    (&ticker, &symbol),
                    feed,
                    &interval,
                    &mut stop,
                    cfg,
                )
                .await
            }
            Err(e) => {
                return log(
                    &ctx,
                    Level::Error,
                    format!("канал {name} для окна не пишется: {e}"),
                )
                .await;
            }
        };
        let r = match end {
            End::Stop => return,
            End::Broken(r) => r,
        };
        let Ok(pause) = ctx.int("venue.ws_reconnect_pause_ms") else {
            let m = format!(
                "WebSocket {name}: {r}; не задана venue.ws_reconnect_pause_ms — канал окна остановлен"
            );
            return log(&ctx, Level::Error, m).await;
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
    (ticker, symbol): (&str, &str),
    feed: Feed,
    interval: &str,
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
    let payload = feed.payload(symbol, interval);
    let sub = depth::request(feed.name(), "subscribe", payload.clone(), 0);
    if let Err(e) = sink.send(Message::Text(sub.into())).await {
        return End::Broken(format!("отправка подписки: {e}"));
    }
    let mut ping = tokio::time::interval(cfg.ping);
    let mut last_rx = Instant::now();
    let mut ver = Versions::default();
    let kind = if feed == Feed::Trades {
        TapStream::Trades
    } else {
        TapStream::Diff
    };
    loop {
        tokio::select! {
            _ = stop.changed() => {
                let unsub = depth::request(feed.name(), "unsubscribe", payload.clone(), 0);
                let _ = sink.send(Message::Text(unsub.into())).await;
                let _ = sink.close().await;
                return End::Stop;
            }
            _ = ping.tick() => {
                if let Err(e) = sink.send(Message::Text(depth::ping().into())).await {
                    return End::Broken(format!("отправка spot.ping: {e}"));
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
                    Message::Text(t) => match depth::parse(&t) {
                        Msg::Ack { error: Some(err), .. } => {
                            log(ctx, Level::Error, format!("WebSocket {name}: отказ подписки: {err}")).await;
                        }
                        Msg::Upgrade => return End::Broken("spot.system: сервер обновляется (upgrade)".into()),
                        Msg::Pong | Msg::Ack { .. } => {}
                        _ => {
                            let need = feed == Feed::Diff && ver.need_snapshot(&t);
                            ctx.taps.push(ID, ticker, kind, name, rx_ns, || TapData::Text(t.to_string()));
                            if need {
                                super::tap_snap::take(ctx, ticker, symbol, name, &mut ver).await;
                            }
                        }
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
pub async fn log(ctx: &Ctx, level: Level, msg: String) {
    let _ = ctx
        .journal
        .write(level, "venue.ws", Some(ID), msg, None)
        .await;
}

//! Файл: venues/mexc/feed.rs
//! Что делает: соединение окна (решение 125) для одной монеты и одного канала на время окна — лента сделок или
//! изменения стакана всей глубины: подключение к venue.ws_url, {"method":"SUBSCRIPTION"}, свой {"method":"PING"} раз в
//! venue.ws_ping_ms, Pong на Ping-кадр, каждый двоичный кадр (protobuf как есть) — в приёмник окна (ctx.taps) со временем
//! получения; обрыв — журнал, пауза venue.ws_reconnect_pause_ms, переподключение; окно закрыто — {"method":
//! "UNSUBSCRIPTION"} и закрытие. У изменений стакана — ещё снимок REST (tap_snap.rs) после первого кадра и при разрыве
//! версий; снимок тоже пишется в окно. Своё соединение на канал монеты: одна подписка (предел MEXC — 30 на соединение).
//! Кто вызывает: tap.rs (запуск при открытии окна).
//! От чего зависит: ws.rs (Cfg, config), depth.rs (parse_text), tap_snap.rs, ctx.rs, gateway/tap.rs.
//! Настройки: venue.ws_url, venue.ws_silence_ms, venue.ws_ping_ms, venue.ws_reconnect_pause_ms, http.timeout_ms (через
//! ws::config); снимок — venue.window_depth_limit (tap_snap.rs).
//!
//! Официально (https://www.mexc.com/api-docs/spot-v3/websocket-market-streams/ и исходник mexcdevelop/apidocs, ветка
//! spot_v3_en): Trade Streams — «spot@public.aggre.deals.v3.api.pb@(100ms|10ms)@<symbol>», «raw trade information for
//! each fill»; Diff.Depth Stream — «spot@public.aggre.depth.v3.api.pb@(100ms|10ms)@<symbol>», fromVersion/toVersion;
//! «How to Properly Maintain a Local Copy of the Order Book» — подписка, затем снимок REST /api/v3/depth, «fromVersion
//! of each new push message should be exactly equal to the toVersion + 1 of the previous message. Otherwise … reinitia-
//! lization from step 2 is required». Отписка — {"method":"UNSUBSCRIPTION","params":[канал]}. Не больше 30 подписок на
//! соединение; без подписки соединение закрывается через 30 с, без данных — через минуту.

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use tokio::sync::watch;
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::Message;

use crate::gateway::tap::{TapData, TapStream, now_ns};
use crate::storage::journal::Level;

use super::ctx::{Ctx, ID};
use super::depth::{self, Msg};
use super::tap_snap::Versions;
use super::ws::{Cfg, config};

/// Канал окна.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Feed {
    /// Лента сделок (интервал — venue.trades_interval).
    Trades,
    /// Изменения стакана всей глубины (интервал — venue.window_depth_interval).
    Diff,
}

impl Feed {
    /// Канал по документации. Принимает: интервал (10ms / 100ms), символ MEXC.
    pub fn channel(self, interval: &str, symbol: &str) -> String {
        match self {
            Feed::Trades => format!("spot@public.aggre.deals.v3.api.pb@{interval}@{symbol}"),
            Feed::Diff => format!("spot@public.aggre.depth.v3.api.pb@{interval}@{symbol}"),
        }
    }
}

enum End {
    Stop,
    Broken(String),
}

/// Работает, пока окно открыто. Принимает: окружение, тикер, символ, канал, вид, выключатель окна. Ошибок нет.
pub async fn run(
    ctx: Ctx,
    ticker: String,
    symbol: String,
    ch: String,
    feed: Feed,
    mut stop: watch::Receiver<bool>,
) {
    let name = format!("window {ch}");
    while !*stop.borrow() {
        let end = match config(&ctx) {
            Ok(cfg) => session(&ctx, &name, (&ticker, &symbol), &ch, feed, &mut stop, cfg).await,
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
        let r = match end {
            End::Stop => return,
            End::Broken(r) => r,
        };
        let Ok(pause) = ctx.int("venue.ws_reconnect_pause_ms") else {
            log(&ctx, Level::Error, format!("WebSocket {name}: {r}; не задана venue.ws_reconnect_pause_ms — канал окна остановлен")).await;
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

/// Одна сессия канала. Принимает: … монету (тикер, символ MEXC) … Возвращает причину окончания.
async fn session(
    ctx: &Ctx,
    name: &str,
    (ticker, symbol): (&str, &str),
    ch: &str,
    feed: Feed,
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
    let sub = json!({ "method": "SUBSCRIPTION", "params": [ch] });
    if let Err(e) = sink.send(Message::Text(sub.to_string().into())).await {
        return End::Broken(format!("отправка подписки: {e}"));
    }
    let mut ping = tokio::time::interval(cfg.ping);
    let mut last_rx = Instant::now();
    // Версии изменений стакана: когда нужен снимок REST (новая сессия — снимок заново).
    let mut ver = Versions::default();
    let stream_kind = match feed {
        Feed::Trades => TapStream::Trades,
        Feed::Diff => TapStream::Diff,
    };
    loop {
        tokio::select! {
            _ = stop.changed() => {
                let unsub = json!({ "method": "UNSUBSCRIPTION", "params": [ch] });
                let _ = sink.send(Message::Text(unsub.to_string().into())).await;
                let _ = sink.close().await;
                return End::Stop;
            }
            _ = ping.tick() => {
                let p = json!({ "method": "PING" });
                if let Err(e) = sink.send(Message::Text(p.to_string().into())).await {
                    return End::Broken(format!("отправка PING: {e}"));
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
                    // В соединении одна подписка — каждый двоичный кадр относится к этому каналу монеты.
                    Message::Binary(b) => {
                        let need = feed == Feed::Diff && ver.need_snapshot(&b);
                        ctx.taps.push(ID, ticker, stream_kind, name, rx_ns, || TapData::Binary(b.to_vec()));
                        if need {
                            super::tap_snap::take(ctx, ticker, symbol, name, &mut ver).await;
                        }
                    }
                    Message::Text(t) => {
                        if let Msg::Ack { code, msg } = depth::parse_text(&t)
                            && code != 0
                        {
                            log(ctx, Level::Error, format!("WebSocket {name}: отказ подписки {ch}, код {code}: {msg}")).await;
                        }
                    }
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

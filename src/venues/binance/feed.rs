//! Файл: venues/binance/feed.rs
//! Что делает: соединение окна (решение 125) для одной монеты и одного потока на время окна — сделки <symbol>@trade
//! или изменения стакана <symbol>@depth[@100ms]: сырой поток /ws/<stream> (подписки сообщением не нужно), Pong на
//! Ping, каждый кадр — в приёмник окна (ctx.taps) со временем получения; обрыв — журнал, пауза
//! venue.ws_reconnect_pause_ms, заново; окно закрыто — закрытие соединения. У изменений стакана — снимок REST
//! /api/v3/depth в начале сессии и при разрыве номеров (U > прежний u + 1).
//! Кто вызывает: tap.rs.
//! От чего зависит: tap_decode.rs (номера), rest.rs, ctx.rs, gateway/tap.rs.
//! Настройки: venue.ws_url, venue.ws_silence_ms, venue.ws_reconnect_pause_ms, venue.window_depth_limit [binance]
//! (1..5000), http.timeout_ms.
//!
//! Официально (web-socket-streams): сырой поток «/ws/<streamName>»; «How to manage a local order book correctly» —
//! снимок GET /api/v3/depth?symbol=&limit= (limit до 5000; вес 5 / 25 / 50 / 250 по limit), события с u ≤ lastUpdateId
//! отбросить, разрыв (U > прежний u + 1) — пересобрать.

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::sync::watch;
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::Message;

use crate::gateway::tap::{TapData, TapStream, now_ns};
use crate::storage::journal::Level;

use super::ctx::{Ctx, ID};

/// Наибольший limit снимка — официально 5000.
pub const MAX_SNAPSHOT: u64 = 5000;

/// Работает, пока окно открыто. Принимает: окружение, тикер, символ, имя потока, поток окна, выключатель.
pub async fn run(
    ctx: Ctx,
    ticker: String,
    symbol: String,
    stream: String,
    kind: TapStream,
    mut stop: watch::Receiver<bool>,
) {
    let name = format!("window {stream}");
    while !*stop.borrow() {
        let (url, silence, timeout) = match (
            ctx.text("venue.ws_url"),
            ctx.int("venue.ws_silence_ms"),
            ctx.http_timeout(),
        ) {
            (Ok(u), Ok(s), Ok(t)) => (u, Duration::from_millis(s), t),
            (Err(e), _, _) | (_, Err(e), _) | (_, _, Err(e)) => {
                let m = format!("поток {stream} для окна не пишется: {}", e.message);
                ctx.log(Level::Error, "venue.ws", m).await;
                return;
            }
        };
        let url = format!("{}/ws/{stream}", url.trim_end_matches('/'));
        let why =
            match tokio::time::timeout(timeout, tokio_tungstenite::connect_async(url.as_str()))
                .await
            {
                Err(_) => "не удалось подключиться за отведённое время".to_string(),
                Ok(Err(e)) => format!("подключение: {e}"),
                Ok(Ok((ws, _))) => {
                    match session(
                        &ctx,
                        ws,
                        (&name, &ticker, &symbol),
                        kind,
                        silence,
                        &mut stop,
                    )
                    .await
                    {
                        None => return,
                        Some(w) => w,
                    }
                }
            };
        let Ok(pause) = ctx.int("venue.ws_reconnect_pause_ms") else {
            let m = format!(
                "WebSocket {name}: {why}; нет venue.ws_reconnect_pause_ms — поток окна остановлен"
            );
            ctx.log(Level::Error, "venue.ws", m).await;
            return;
        };
        let m = format!("WebSocket {name}: {why}; переподключение (в записи окна — пропуск)");
        ctx.log(Level::Warn, "venue.ws", m).await;
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_millis(pause)) => {}
            _ = stop.changed() => return,
        }
    }
}

/// Соединение WebSocket.
type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// Одна сессия. Возвращает: None — окно закрыто; Some(причина) — обрыв.
async fn session(
    ctx: &Ctx,
    ws: Ws,
    (name, ticker, symbol): (&str, &str, &str),
    kind: TapStream,
    silence: Duration,
    stop: &mut watch::Receiver<bool>,
) -> Option<String> {
    let (mut sink, mut stream) = ws.split();
    let mut last_rx = Instant::now();
    // Номер, до которого стакан известен (снимок или последний кадр); None — снимка ещё нет.
    let mut cur: Option<i64> = None;
    let mut retry_at = Instant::now();
    loop {
        tokio::select! {
            _ = stop.changed() => {
                let _ = sink.close().await;
                return None;
            }
            _ = tokio::time::sleep_until(last_rx + silence) => {
                return Some(format!("нет кадров дольше {} мс", silence.as_millis()));
            }
            next = stream.next() => {
                let f = match next {
                    None => return Some("соединение закрыто сервером".into()),
                    Some(Err(e)) => return Some(format!("ошибка чтения: {e}")),
                    Some(Ok(f)) => f,
                };
                let rx_ns = now_ns();
                last_rx = Instant::now();
                let t = match f {
                    Message::Text(t) => t,
                    Message::Ping(p) => {
                        if let Err(e) = sink.send(Message::Pong(p)).await {
                            return Some(format!("отправка Pong: {e}"));
                        }
                        continue;
                    }
                    _ => continue,
                };
                ctx.taps.push(ID, ticker, kind, name, rx_ns, || TapData::Text(t.to_string()));
                if kind != TapStream::Diff {
                    continue;
                }
                let Some((from, to)) = super::tap_decode::diff_range(&t) else { continue };
                let need = match cur {
                    Some(c) if to <= c => false,
                    Some(c) if from <= c + 1 => { cur = Some(to); false }
                    _ => true,
                };
                // Неудавшийся снимок — повтор не раньше venue.ws_reconnect_pause_ms (не долбить REST на каждом кадре).
                if need && Instant::now() >= retry_at {
                    cur = snapshot(ctx, ticker, symbol, name).await;
                    if cur.is_none() {
                        let ms = ctx.int("venue.ws_reconnect_pause_ms").unwrap_or(0);
                        retry_at = Instant::now() + Duration::from_millis(ms);
                    }
                }
            }
        }
    }
}

/// Снимок REST в окно (поток Snapshot). Возвращает: его lastUpdateId; None — не получен (в журнал). Ошибок нет.
async fn snapshot(ctx: &Ctx, ticker: &str, symbol: &str, conn: &str) -> Option<i64> {
    let limit = ctx
        .int("venue.window_depth_limit")
        .ok()
        .filter(|n| (1..=MAX_SNAPSHOT).contains(n))?;
    let q = vec![("symbol", symbol.to_string()), ("limit", limit.to_string())];
    match super::rest::public(ctx, "/api/v3/depth", q).await {
        Ok(v) => {
            let id = super::tap_decode::snapshot_id(&v);
            let text = v.to_string();
            ctx.taps
                .push(ID, ticker, TapStream::Snapshot, conn, now_ns(), || {
                    TapData::Text(text)
                });
            id
        }
        Err(e) => {
            let m = format!("окно {ticker}: снимок стакана не получен: {}", e.message);
            ctx.log(Level::Warn, "venue.rest", m).await;
            None
        }
    }
}

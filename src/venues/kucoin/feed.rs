//! Файл: venues/kucoin/feed.rs
//! Что делает: соединение окна (решение 125) для одной монеты и одного канала на время окна — сделки /market/match или
//! изменения стакана /market/level2: свой публичный токен, welcome, подписка, ping по pingInterval, каждый кадр канала —
//! в приёмник окна (ctx.taps) со временем получения; обрыв — журнал, пауза venue.ws_reconnect_pause_ms, заново; окно
//! закрыто — unsubscribe и закрытие. У изменений стакана — снимок REST part orderbook в начале сессии и при разрыве
//! номеров (sequenceStart > прежний sequenceEnd + 1).
//! Кто вызывает: tap.rs.
//! От чего зависит: bullet.rs, ws_conn.rs (open), tap_decode.rs (номера), rest.rs, ctx.rs, gateway/tap.rs.
//! Настройки: venue.ws_reconnect_pause_ms, venue.window_depth_limit [kucoin] (20 или 100), http.timeout_ms.
//!
//! Официально: темы «/market/match:{symbol}» и «/market/level2:{symbol}» (спецификация SDK, x-topic); снимок — Get Part
//! OrderBook GET /api/v1/market/orderbook/level2_{size} (Public, вес 2; size «optional value: 20, 100»)
//! https://www.kucoin.com/docs-new/api-3470165; отписка — type unsubscribe (introduction).

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use tokio::sync::watch;
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::Message;

use crate::gateway::tap::{TapData, TapStream, now_ns};
use crate::storage::journal::Level;

use super::ctx::{Ctx, ID};
use super::reply::Pool;

/// Допустимые глубины снимка — официально «optional value: 20, 100».
pub const SNAPSHOT_SIZES: [u64; 2] = [20, 100];

/// Работает, пока окно открыто. Принимает: окружение, тикер, символ, тему, поток окна, выключатель. Ошибок нет.
pub async fn run(
    ctx: Ctx,
    ticker: String,
    symbol: String,
    topic: String,
    kind: TapStream,
    mut stop: watch::Receiver<bool>,
) {
    let name = format!("window {topic}");
    while !*stop.borrow() {
        let why = match (ctx.http_timeout(), super::bullet::get(&ctx, false).await) {
            (Ok(t), Ok(token)) => match super::ws_conn::open(&token, t).await {
                Ok(ws) => match session(
                    &ctx,
                    ws,
                    (&name, &ticker, &symbol),
                    &topic,
                    kind,
                    &token,
                    &mut stop,
                )
                .await
                {
                    None => return,
                    Some(w) => w,
                },
                Err(e) => e,
            },
            (Err(e), _) => {
                ctx.log(
                    Level::Error,
                    "venue.ws",
                    format!("канал {topic} для окна не пишется: {}", e.message),
                )
                .await;
                return;
            }
            (_, Err(e)) => format!("токен bullet-public: {}", e.message),
        };
        let Ok(pause) = ctx.int("venue.ws_reconnect_pause_ms") else {
            ctx.log(Level::Error, "venue.ws", format!("WebSocket {name}: {why}; нет venue.ws_reconnect_pause_ms — канал окна остановлен")).await;
            return;
        };
        ctx.log(
            Level::Warn,
            "venue.ws",
            format!("WebSocket {name}: {why}; переподключение (в записи окна — пропуск)"),
        )
        .await;
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_millis(pause)) => {}
            _ = stop.changed() => return,
        }
    }
}

/// Одна сессия. Возвращает: None — окно закрыто; Some(причина) — обрыв.
async fn session(
    ctx: &Ctx,
    ws: super::ws_conn::Ws,
    (name, ticker, symbol): (&str, &str, &str),
    topic: &str,
    kind: TapStream,
    token: &super::bullet::Token,
    stop: &mut watch::Receiver<bool>,
) -> Option<String> {
    let (mut sink, mut stream) = ws.split();
    let sub = json!({"id": "w1", "type": "subscribe", "topic": topic, "response": true});
    if let Err(e) = sink.send(Message::Text(sub.to_string().into())).await {
        return Some(format!("отправка подписки: {e}"));
    }
    let mut ping = tokio::time::interval_at(Instant::now() + token.ping, token.ping);
    let silence = token.ping + token.ping_timeout;
    let mut last_rx = Instant::now();
    // Номер, до которого стакан известен (снимок или последний кадр); None — снимка ещё нет.
    let mut cur: Option<i64> = None;
    let mut retry_at = Instant::now();
    loop {
        tokio::select! {
            _ = stop.changed() => {
                let un = json!({"id": "w2", "type": "unsubscribe", "topic": topic, "response": true});
                let _ = sink.send(Message::Text(un.to_string().into())).await;
                let _ = sink.close().await;
                return None;
            }
            _ = ping.tick() => {
                let p = json!({"id": now_ns().to_string(), "type": "ping"});
                if let Err(e) = sink.send(Message::Text(p.to_string().into())).await {
                    return Some(format!("отправка ping: {e}"));
                }
            }
            _ = tokio::time::sleep_until(last_rx + silence) => return Some(format!("нет кадров дольше {} мс", silence.as_millis())),
            next = stream.next() => {
                let f = match next {
                    None => return Some("соединение закрыто сервером".into()),
                    Some(Err(e)) => return Some(format!("ошибка чтения: {e}")),
                    Some(Ok(f)) => f,
                };
                let rx_ns = now_ns();
                last_rx = Instant::now();
                let Message::Text(t) = f else { continue };
                let range = super::tap_decode::diff_range(&t);
                let is_data = range.is_some() || t.contains("\"type\":\"message\"");
                if is_data {
                    ctx.taps.push(ID, ticker, kind, name, rx_ns, || TapData::Text(t.to_string()));
                }
                if kind == TapStream::Diff && let Some((from, to)) = range {
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
}

/// Снимок REST в окно (поток Snapshot). Возвращает: его sequence; None — не получен (в журнал). Ошибок нет.
async fn snapshot(ctx: &Ctx, ticker: &str, symbol: &str, conn: &str) -> Option<i64> {
    let size = ctx
        .int("venue.window_depth_limit")
        .ok()
        .filter(|s| SNAPSHOT_SIZES.contains(s));
    let Some(size) = size else {
        ctx.log(
            Level::Warn,
            "venue.rest",
            format!("окно {ticker}: venue.window_depth_limit не 20/100 — снимка нет"),
        )
        .await;
        return None;
    };
    let path = format!("/api/v1/market/orderbook/level2_{size}");
    let q = vec![("symbol", symbol.to_string())];
    match super::rest::public(ctx, &path, q, Pool::Public).await {
        Ok(v) => {
            let seq = v
                .get("sequence")
                .and_then(|s| s.as_str())
                .and_then(|s| s.parse().ok());
            let text = v.to_string();
            ctx.taps
                .push(ID, ticker, TapStream::Snapshot, conn, now_ns(), || {
                    TapData::Text(text)
                });
            seq
        }
        Err(e) => {
            ctx.log(
                Level::Warn,
                "venue.rest",
                format!("окно {ticker}: снимок стакана не получен: {}", e.message),
            )
            .await;
            None
        }
    }
}

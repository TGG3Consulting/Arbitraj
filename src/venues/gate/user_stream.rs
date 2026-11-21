//! Файл: venues/gate/user_stream.rs
//! Что делает: поток аккаунта Gate — изменения баланса спота (spot.balances) для зачисления переводов без опроса
//! статуса (решение 76). Включено venue.balance_events [gate] → соединение с venue.ws_url, подписка с подписью
//! (account_update.rs), свой spot.ping раз в venue.ws_ping_ms, Pong на Ping-кадр; обрыв — пауза
//! venue.ws_reconnect_pause_ms и заново; изменение настроек — заново. Ключа потока (listenKey) у Gate нет — подпись в
//! самой подписке. Ключ и подпись в журнал и сырые сообщения не попадают (отправленное не пишется).
//! Хеша транзакции в событии нет: закрывать им зачисление или нет — настройка ядра transfer.credit_by_event [gate]
//! (до проверки на живой записи не включать).
//! Кто вызывает: subs.rs (запускается один раз с первой подпиской на рынок, живёт, пока жив канал ядра).
//! От чего зависит: account_update.rs, depth.rs (ответы, пинг), ctx.rs, settings/live.rs (Watch).
//! Настройки: venue.balance_events, venue.ws_url, venue.ws_silence_ms, venue.ws_reconnect_pause_ms, venue.ws_ping_ms,
//! http.timeout_ms.
//!
//! Официально (Spot WebSocket v4 https://www.gate.com/docs/developers/apiv4/ws/en/): spot.balances — «This channel
//! requires authentication»; ключу нужно право чтения спота, IP — из белого списка ключа, если он включён.

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::Message;

use crate::gateway::events::MarketEvent;
use crate::settings::live::Watch;
use crate::storage::journal::Level;

use super::account_update::{self, Frame};
use super::ctx::{Ctx, ID};
use super::depth::{self, Msg};

/// Имя соединения в сыром журнале.
const NAME: &str = "account";

/// Работает, пока жив канал ядра. Ошибок не возвращает: всё — в журнал.
pub async fn run(ctx: Ctx, out: mpsc::Sender<MarketEvent>) {
    let names = [
        "venue.balance_events",
        "venue.ws_url",
        "venue.ws_silence_ms",
        "http.timeout_ms",
        "venue.ws_ping_ms",
    ];
    let mut changes = Watch::new(
        ctx.settings.subscribe(),
        names.iter().map(|n| n.to_string()).collect(),
    );
    let mut told = String::new();
    while !out.is_closed() {
        let (why, retry) = match ctx.flag("venue.balance_events") {
            Ok(true) => tokio::select! {
                w = session(&ctx, &out) => match w { Some(w) => (w, true), None => return },
                c = changes.next() => match c {
                    None => return,
                    Some(c) => (format!("{} изменена — поток аккаунта заново", c.name), true),
                },
            },
            Ok(false) => (
                "venue.balance_events = нет — поток аккаунта выключен".into(),
                false,
            ),
            Err(e) => (e.message, false),
        };
        if why != told {
            let level = if retry { Level::Warn } else { Level::Info };
            let _ = ctx
                .journal
                .write(
                    level,
                    "venue.user_stream",
                    Some(ID),
                    format!("Поток аккаунта: {why}"),
                    None,
                )
                .await;
            told = why;
        }
        let pause = ctx
            .int("venue.ws_reconnect_pause_ms")
            .ok()
            .filter(|_| retry);
        let wait = async {
            match pause {
                Some(ms) => tokio::time::sleep(Duration::from_millis(ms)).await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            _ = wait => {}
            c = changes.next() => if c.is_none() { return },
        }
    }
}

/// Сессия до обрыва. Возвращает: Some(причина) — переподключиться; None — канал ядра закрыт.
async fn session(ctx: &Ctx, out: &mpsc::Sender<MarketEvent>) -> Option<String> {
    let cfg = || -> Result<_, String> {
        let ms = |n: &str| ctx.int(n).map(Duration::from_millis).map_err(|e| e.message);
        Ok((
            ctx.text("venue.ws_url").map_err(|e| e.message)?,
            ms("venue.ws_silence_ms")?,
            ms("venue.ws_ping_ms")?,
            ctx.http_timeout().map_err(|e| e.message)?,
        ))
    };
    let (url, silence, ping_every, connect) = match cfg() {
        Ok(c) => c,
        Err(e) => return Some(e),
    };
    let sub = match account_update::subscribe(ctx) {
        Ok(s) => s,
        Err(e) => return Some(e.message),
    };
    let ws =
        match tokio::time::timeout(connect, tokio_tungstenite::connect_async(url.as_str())).await {
            Err(_) => return Some("не удалось подключиться за отведённое время".into()),
            Ok(Err(e)) => return Some(format!("подключение: {e}")),
            Ok(Ok((ws, _))) => ws,
        };
    let (mut sink, mut stream) = ws.split();
    if let Err(e) = sink.send(Message::Text(sub.into())).await {
        return Some(format!("отправка подписки: {e}"));
    }
    let mut ping = tokio::time::interval(ping_every);
    let mut last_rx = Instant::now();
    loop {
        tokio::select! {
            _ = ping.tick() => {
                if let Err(e) = sink.send(Message::Text(depth::ping().into())).await {
                    return Some(format!("отправка spot.ping: {e}"));
                }
            }
            _ = tokio::time::sleep_until(last_rx + silence) => {
                return Some(format!("нет кадров дольше {} мс", silence.as_millis()));
            }
            next = stream.next() => {
                let frame = match next {
                    None => return Some("соединение закрыто сервером".into()),
                    Some(Err(e)) => return Some(format!("ошибка чтения: {e}")),
                    Some(Ok(f)) => f,
                };
                last_rx = Instant::now();
                let t = match frame {
                    Message::Text(t) => t,
                    Message::Ping(p) => {
                        if let Err(e) = sink.send(Message::Pong(p)).await {
                            return Some(format!("отправка Pong: {e}"));
                        }
                        continue;
                    }
                    Message::Close(c) => return Some(format!("сервер закрыл соединение: {c:?}")),
                    _ => continue,
                };
                // Урок 18: ключ не попадает в сырой журнал, даже если сервер повторит запрос в ответе.
                let t = match ctx.keys() {
                    Ok((k, _)) if !k.is_empty() && t.contains(k.as_str()) => t.replace(k.as_str(), "***"),
                    _ => t.to_string(),
                };
                ctx.raw.write(ID, NAME, &t);
                match depth::parse(&t) {
                    Msg::Ack { error: Some(err), .. } => return Some(format!("подписка spot.balances отклонена: {err}")),
                    Msg::Upgrade => return Some("spot.system: сервер обновляется (upgrade)".into()),
                    _ => {}
                }
                if !on_frame(ctx, out, &t).await {
                    return None;
                }
            }
        }
    }
}

/// Кадр → журнал и события ядру. Возвращает: false — канал ядра закрыт. Ошибок нет.
async fn on_frame(ctx: &Ctx, out: &mpsc::Sender<MarketEvent>, text: &str) -> bool {
    let (level, msg, list) = match account_update::parse(text, &ctx.venue()) {
        Frame::Other => return true,
        Frame::Bad(why) => (
            Level::Error,
            format!("Изменение баланса не разобрано: {why}"),
            vec![],
        ),
        Frame::Update { reasons, changes } => {
            let what: Vec<String> = changes
                .iter()
                .map(|c| format!("{} {:+}", c.asset.ticker, c.delta))
                .collect();
            (
                Level::Info,
                format!(
                    "Изменение баланса change_type={}: {}",
                    reasons.join(","),
                    what.join(", ")
                ),
                changes,
            )
        }
    };
    let data = Some(serde_json::json!({ "raw": text }));
    let _ = ctx
        .journal
        .write(level, "venue.user_stream", Some(ID), msg, data)
        .await;
    for c in list {
        if out.send(MarketEvent::Balance(c)).await.is_err() {
            return false;
        }
    }
    true
}

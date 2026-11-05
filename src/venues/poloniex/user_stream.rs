//! Файл: venues/poloniex/user_stream.rs
//! Что делает: поток аккаунта Poloniex — изменения баланса спота (канал balances приватного WebSocket) для зачисления
//! переводов без опроса статуса (решение 76). Включено venue.balance_events [poloniex] → соединение с
//! venue.ws_private_url, вход auth с подписью (account_update.rs), подписка balances, опорные остатки — REST балансов
//! спота после подписки, свой ping раз в venue.ws_ping_ms; обрыв — пауза venue.ws_reconnect_pause_ms и заново;
//! изменение настроек — заново. Ключ и подпись в журнал и сырые сообщения не попадают (отправленное не пишется).
//! Хеша транзакции в уведомлении нет: закрывать им зачисление или нет — настройка ядра transfer.credit_by_event
//! [poloniex] (до проверки на живой записи не включать).
//! Кто вызывает: subs.rs (запускается один раз с первой подпиской на рынок, живёт, пока жив канал ядра).
//! От чего зависит: account_update.rs, account.rs (опорные остатки), depth.rs (ping), ws.rs (ping_silence), ctx.rs,
//! settings/live.rs (Watch).
//! Настройки: venue.balance_events, venue.ws_private_url, venue.ws_silence_ms, venue.ws_reconnect_pause_ms,
//! venue.ws_ping_ms, http.timeout_ms.
//!
//! Официально (Spot → WebSocket, https://api-docs.poloniex.com/spot/websocket/): приватный адрес
//! «wss://ws.poloniex.com/ws/private»; «Private channel data is available only after a successful authentication»;
//! ping — как у публичного (каждые 30 с, иначе сессия закрывается без уведомления).

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::Message;

use crate::gateway::events::MarketEvent;
use crate::settings::live::Watch;
use crate::storage::journal::Level;

use super::account_update::{self, Frame, Totals};
use super::ctx::{Ctx, ID};
use super::{account, depth, ws};

/// Имя соединения в сыром журнале.
const NAME: &str = "account";
/// Код записей журнала.
const CODE: &str = "venue.user_stream";

/// Работает, пока жив канал ядра. Ошибок не возвращает: всё — в журнал.
pub async fn run(ctx: Ctx, out: mpsc::Sender<MarketEvent>) {
    let names = [
        "venue.balance_events",
        "venue.ws_private_url",
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
            ctx.log(level, CODE, format!("Поток аккаунта: {why}")).await;
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
        let (ping, silence) = ws::ping_silence(ctx)?;
        Ok((
            ctx.text("venue.ws_private_url").map_err(|e| e.message)?,
            ping,
            silence,
            ctx.http_timeout().map_err(|e| e.message)?,
        ))
    };
    let (url, ping_every, silence, connect) = match cfg() {
        Ok(c) => c,
        Err(e) => return Some(e),
    };
    let auth = match account_update::auth(ctx) {
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
    if let Err(e) = sink.send(Message::Text(auth.into())).await {
        return Some(format!("отправка auth: {e}"));
    }
    let mut totals = Totals::default();
    let mut ping = tokio::time::interval_at(Instant::now() + ping_every, ping_every);
    let mut last_rx = Instant::now();
    loop {
        tokio::select! {
            _ = ping.tick() => {
                if let Err(e) = sink.send(Message::Text(depth::ping().into())).await {
                    return Some(format!("отправка ping: {e}"));
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
                    Ok(k) if !k.key.is_empty() && t.contains(k.key.as_str()) => t.replace(k.key.as_str(), "***"),
                    _ => t.to_string(),
                };
                ctx.raw.write(ID, NAME, &t);
                match account_update::parse(&t) {
                    Frame::Auth(Err(why)) => return Some(format!("вход auth отклонён: {why}")),
                    Frame::Auth(Ok(())) => {
                        if let Err(e) = sink.send(Message::Text(account_update::subscribe().into())).await {
                            return Some(format!("отправка подписки balances: {e}"));
                        }
                        // Опорные остатки — после подписки: изменение, случившееся до неё, уже в остатке REST.
                        match account::balances(ctx).await {
                            Ok(rows) => totals.seed(&rows),
                            Err(e) => return Some(format!("опорные остатки спота: {}", e.message)),
                        }
                    }
                    Frame::Error(text) => return Some(format!("ошибка Poloniex: {text}")),
                    Frame::Bad(why) => {
                        let data = Some(serde_json::json!({ "raw": t }));
                        let m = format!("Изменение баланса не разобрано: {why}");
                        let _ = ctx.journal.write(Level::Error, CODE, Some(ID), m, data).await;
                    }
                    Frame::Balances(rows) => {
                        for r in rows {
                            let Some(c) = totals.apply(&ctx.venue(), &r) else { continue };
                            let m = format!("Изменение баланса eventType={}: {} {:+}", r.event_type, c.asset.ticker, c.delta);
                            ctx.log(Level::Info, CODE, m).await;
                            if out.send(MarketEvent::Balance(c)).await.is_err() {
                                return None;
                            }
                        }
                    }
                    Frame::Other => {}
                }
            }
        }
    }
}

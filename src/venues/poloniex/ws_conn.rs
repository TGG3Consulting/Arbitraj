//! Файл: venues/poloniex/ws_conn.rs
//! Что делает: одна сессия публичного WebSocket Poloniex: подключение, подписка каждой пары на два канала — book
//! (снимки N уровней, страховка) и book_lv2 (снимок 20 уровней + изменения, быстрый стакан; решение 424) — по одному
//! символу в сообщении с паузой venue.ws_subs_per_sec, свой ping раз в venue.ws_ping_ms, тишина дольше
//! venue.ws_silence_ms — обрыв. Стаканы обоих каналов — ws_book.rs (выбор: fast_book.rs). Сырые кадры стакана — в
//! журнал только при raw.book_frames = да (решение 109).
//! Кто вызывает: ws.rs.
//! От чего зависит: depth.rs, lv2.rs, ws_book.rs, ws_book_sync.rs, book.rs, ctx.rs, tokio-tungstenite,
//! gateway/feed_reg.rs (решение 202).
//! Настройки: приходят из ws.rs (Cfg); raw.book_frames [poloniex].
//!
//! Решение 202 (для всех бирж, Тигран 08.11): каждый кадр отмечается в учёте свежести (gateway/feed_reg.rs) — кадр
//! соединения здесь, кадр пары — ws_book.rs (новые данные пары: отданный или подтверждённый стакан, неразборчивый
//! кадр с символом); id кадра не передаётся (решение 213: в ответе REST стакана номера нет — подтверждение тихой пары
//! по содержимому, book_print.rs). Команды сторожа ядра: «переподписать пару» — отписка обоих каналов
//! ({"event": "unsubscribe", "channel": [канал], "symbols": [символ]}), затем подписка обоих (Spot → WebSocket:
//! «Unsubscribe» — тот же channel и symbols); все сообщения — в очередь отправки с темпом venue.ws_subs_per_sec (в
//! счёт «500 requests per second»), соседи не трогаются; прежние id и стаканы пары забываются. Ответ на отписку без
//! символа («Not subscribed» и др.) — как любая ошибка Poloniex, в журнал. «Переподключить соединение» — сессия
//! заканчивается, ws.rs подключается заново с теми же парами (после переподключения подписки идут той же очередью с
//! темпом venue.ws_subs_per_sec — массовая пересинхронизация без всплеска; REST не нужен).
//!
//! Решение 109/263 (канал book): снимок с тем же или меньшим id (SeqId) ядру не отдаётся и свежесть пары не продлевает
//! (depth::newer, ws_book.rs). Официально (Spot → WebSocket, Market Data → Book и Book Level 2,
//! https://api-docs.poloniex.com/spot/websocket/market-data): подписка book — {"event": "subscribe", "channel":
//! ["book"], "symbols": [...], "depth": N}; book_lv2 — без depth. Сколько символов в одном сообщении и подписок на
//! соединение, в документации нет — по одному символу в сообщении (урок 12); подписка шлётся только при подключении
//! (урок 14). Решение 424: подписок на пару — две (на соединение вдвое больше, venue.ws_symbols_per_conn — Тигран).

use std::sync::atomic::Ordering;

use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::Message;

use crate::core::types::Asset;
use crate::gateway::events::MarketEvent;
use crate::storage::journal::Level;

use super::book::Books;
use super::ctx::{Ctx, ID};
use super::depth::{self, BOOK, Msg};
use super::lv2;
use super::ws::{Cfg, End};
use super::ws_book::BookConn;
use crate::gateway::feed_health::Cmd;
use crate::gateway::feed_reg::ConnHandle;

/// Учёт свежести соединения и канал команд сторожа (решение 202).
pub struct Watch<'a> {
    pub feed: &'a ConnHandle,
    pub cmd: &'a mut mpsc::UnboundedReceiver<Cmd>,
}

/// Что делает: сообщение подписки на канал book для одного символа. Ошибок нет.
pub fn sub_message(symbol: &str, depth: u64) -> String {
    json!({"event": "subscribe", "channel": [BOOK], "symbols": [symbol], "depth": depth})
        .to_string()
}

/// Что делает: сообщение отписки канала book одного символа (Spot → WebSocket, «Unsubscribe»). Ошибок нет.
pub fn unsub_message(symbol: &str) -> String {
    json!({"event": "unsubscribe", "channel": [BOOK], "symbols": [symbol]}).to_string()
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
    let mut bc = BookConn::new(
        ctx,
        name,
        assets,
        books,
        out,
        w.feed,
        cfg.depth,
        cfg.resync_pause,
    );
    // Очередь отправки берёт с конца: пары — по порядку, у каждой сначала book, затем book_lv2.
    let mut pending: Vec<String> = Vec::with_capacity(assets.len() * 2);
    for (_, s) in assets.iter().rev() {
        pending.push(lv2::sub_message(s));
        pending.push(sub_message(s, cfg.depth));
    }
    let mut sub_tick = tokio::time::interval(cfg.sub_gap);
    let mut ping = tokio::time::interval_at(Instant::now() + cfg.ping, cfg.ping);
    let mut last_rx = Instant::now();
    loop {
        // Решение 424: ближайшая отложенная переподписка book_lv2 (не раньше venue.ws_reconnect_pause_ms на пару).
        let due = bc.next_due();
        tokio::select! {
            _ = sub_tick.tick(), if !pending.is_empty() => {
                let Some(m) = pending.pop() else { continue };
                if let Err(e) = sink.send(Message::Text(m.into())).await {
                    return End::Broken(format!("отправка подписки: {e}"));
                }
            }
            _ = tokio::time::sleep_until(Instant::from_std(due.unwrap_or_else(std::time::Instant::now))), if due.is_some() => {
                bc.release(&mut pending);
            }
            Some(c) = w.cmd.recv() => match c {
                Cmd::Reconnect => return End::Broken("сторож пар: переподключение соединения (решение 202)".into()),
                Cmd::Resub(ticker) => {
                    let Some((_, s)) = assets.iter().find(|(a, _)| a.ticker == ticker) else { continue };
                    // Очередь берёт с конца: сначала отписки, затем подписки обоих каналов.
                    pending.push(lv2::sub_message(s));
                    pending.push(sub_message(s, cfg.depth));
                    pending.push(lv2::unsub_message(s));
                    pending.push(unsub_message(s));
                    bc.forget(s);
                    w.feed.resubscribed(&ticker);
                }
            },
            _ = ping.tick() => {
                if let Err(e) = sink.send(Message::Text(depth::ping().into())).await {
                    return End::Broken(format!("отправка ping: {e}"));
                }
                bc.note();
            }
            _ = tokio::time::sleep_until(last_rx + cfg.silence) => {
                return End::Broken(format!("нет кадров дольше venue.ws_silence_ms ({} мс)", cfg.silence.as_millis()));
            }
            next = stream.next() => {
                let frame = match next {
                    None => return End::Broken("соединение закрыто сервером".into()),
                    Some(Err(e)) => return End::Broken(format!("ошибка чтения: {e}")),
                    Some(Ok(f)) => f,
                };
                last_rx = Instant::now();
                bc.alive.store(chrono::Utc::now().timestamp_millis(), Ordering::Relaxed);
                w.feed.alive();
                let text = match frame {
                    Message::Text(t) => t,
                    Message::Ping(p) => {
                        if let Err(e) = sink.send(Message::Pong(p)).await {
                            return End::Broken(format!("отправка Pong: {e}"));
                        }
                        continue;
                    }
                    Message::Close(c) => return End::Broken(format!("сервер закрыл соединение: {c:?}")),
                    _ => continue,
                };
                let msg = depth::parse(&text);
                let is_book = matches!(
                    &msg,
                    Msg::Books(_) | Msg::Lv2(_) | Msg::Bad { symbol: Some(_), .. } | Msg::BadLv2 { symbol: Some(_), .. }
                );
                if !is_book || matches!(ctx.flag("raw.book_frames"), Ok(true)) {
                    ctx.raw.write(ID, name, &text);
                }
                let open = match msg {
                    Msg::Books(list) => bc.on_books(list, &mut pending),
                    Msg::Lv2(list) => bc.on_lv2(list, &mut pending),
                    Msg::Bad { symbol, reason } => bad(ctx, name, &mut bc, symbol, false, reason, &mut pending).await,
                    Msg::BadLv2 { symbol, reason } => bad(ctx, name, &mut bc, symbol, true, reason, &mut pending).await,
                    // В ошибке нет символа — какая подписка не прошла, неизвестно. Переподключение повторило бы ту же
                    // ошибку по кругу (урок 12): запись в журнал; пара без снимков сама остаётся без котировки (book.rs).
                    Msg::Error(text) => {
                        ctx.log(Level::Error, "venue.ws", format!("WebSocket {name}: ошибка Poloniex: {text}")).await;
                        true
                    }
                    _ => true,
                };
                if !open {
                    return End::Done;
                }
            }
        }
    }
}

/// Неразборчивый кадр стакана: запись в журнал; есть символ — стакан пары по ws_book.rs (book_lv2 — разрыв и
/// переподписка; book — без страховки). Принимает: окружение, имя соединения, стаканы, символ, канал book_lv2 ли,
/// причину, очередь отправки. Возвращает: false — канал ядра закрыт. Ошибок нет.
async fn bad(
    ctx: &Ctx,
    name: &str,
    bc: &mut BookConn<'_>,
    symbol: Option<String>,
    is_lv2: bool,
    reason: String,
    pending: &mut Vec<String>,
) -> bool {
    ctx.log(
        Level::Error,
        "venue.ws",
        format!("WebSocket {name}: кадр не разобран: {reason}"),
    )
    .await;
    match symbol {
        Some(s) => bc.on_bad(&s, is_lv2, &reason, pending),
        None => true,
    }
}

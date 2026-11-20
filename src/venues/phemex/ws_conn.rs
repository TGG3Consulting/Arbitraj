//! Файл: venues/phemex/ws_conn.rs
//! Что делает: одна сессия публичного WebSocket стаканов Phemex: подключение, подписка orderbook.subscribe по одной паре
//! на запрос с паузой venue.ws_subs_per_sec, свой server.ping раз в venue.ws_ping_ms, тишина дольше venue.ws_silence_ms
//! — обрыв; кадры стакана → локальный стакан (local_book.rs) → память и ядру (без ожидания, try_send); разрыв номера —
//! пара недостоверна до следующего снимка; отказ подписки (error в ответе на её id) — пара недостоверна. Сырые кадры
//! стакана — в журнал только при raw.book_frames = да (решение 109).
//! Решение 324: стакан ядру — gateway/core_out_book.rs (канал полон: venue.book_events_keep = да — последний кадр
//! пары ждёт места в общем наборе канала ядра; не задана — прежний сброс и счёт dropped).
//! Решение 427: поток соединения (book_lane.rs Lane) — основной (orderbook.subscribe [symbol]) или запасной
//! (venue.book_reserve: [symbol] или [symbol, true] — «Subscribe full orderBook»); стакан ядру — через выбор
//! book_pick.rs (основной, пока синхронизирован; потеря — сразу запасной); разрыв, неразборчивый кадр пары (локальный
//! стакан пары сброшен до снимка) и отказ подписки — потеря потока пары (Lane::lose), журнал — с прореживанием
//! (book_lane.rs); контрольный снимок, разошедшийся со стаканом в памяти, — счёт (local_book.rs Applied::Differs).
//! Кто вызывает: ws.rs.
//! От чего зависит: depth.rs, local_book.rs, book.rs, symbols.rs, ws.rs (Cfg, End), ctx.rs, tokio-tungstenite,
//! gateway/feed_reg.rs (решение 202).
//! Настройки: из ws.rs (Cfg); raw.book_frames [phemex].
//!
//! Официально (Spot Websocket API, https://phemex-docs.github.io/#subscribe-orderbook-2 и #heartbeat-2): подписка
//! {"id": <id>, "method": "orderbook.subscribe", "params": ["<symbol>"]}; heartbeat {"id": 0, "method":
//! "server.ping", "params": []} → {"error": null, "id": 0, "result": "pong"}. Подписка шлётся только при подключении
//! (урок 14): отписки одной пары нет — смена набора пар = новое соединение (market.rs).
//!
//! Решение 202 (для всех бирж, Тигран 08.11): каждый кадр отмечается в учёте свежести (gateway/feed_reg.rs) — кадр
//! стакана по паре (снимок, изменение, в том числе пропущенный до снимка и с разрывом номера, неразборчивый с символом);
//! sequence не передаётся (решение 213: связь с sequence ответа GET /md/orderbook в документации не описана —
//! подтверждение по содержимому, book_print.rs). Команды сторожа ядра: «переподписать пару» — отписки одной пары у
//! Phemex нет («Unsubscribe OrderBook»: {"id", "method": "orderbook.unsubscribe", "params": []} — «It unsubscribes all
//! orderbook related subscriptions»), поэтому, как у CoinEx, — orderbook.unsubscribe, затем orderbook.subscribe каждой
//! пары соединения с прежними id («On each successful subscription, DataGW will immediately send the current Order Book
//! snapshot») через ту же очередь с темпом venue.ws_subs_per_sec («20 request/s» на соединение); соседи получают
//! новый снимок и начинают прогрев. Очередь подписки не пуста — подписка всех пар уже идёт, повтор не ставится. Ответ с
//! ошибкой на id отписки — в журнал. «Переподключить соединение» — сессия заканчивается, ws.rs подключается заново.

use std::collections::HashMap;
use std::sync::atomic::Ordering;

use chrono::{TimeZone, Utc};
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::Message;

use crate::core::types::{Asset, OrderBook};
use crate::gateway::core_out_book::{BookOut, Fed};
use crate::gateway::events::MarketEvent;
use crate::storage::journal::Level;

use super::book::{Alive, Books};
use super::book_lane::{self, Lane};
use super::ctx::{Ctx, ID};
use super::depth::{self, Msg};
use super::local_book::{Applied, LocalBook};
use super::symbols::Symbols;
use super::ws::{Cfg, End};
use crate::gateway::feed_health::Cmd;
use crate::gateway::feed_reg::ConnHandle;

/// Учёт свежести соединения и канал команд сторожа (решение 202).
pub struct Watch<'a> {
    pub feed: &'a ConnHandle,
    pub cmd: &'a mut mpsc::UnboundedReceiver<Cmd>,
}

/// Пара соединения: монета и масштабы цены и количества.
struct Pair<'a> {
    asset: &'a Asset,
    price_scale: u32,
    qty_scale: u32,
}

/// Сообщение подписки на стакан одной пары. Принимает: id запроса, символ. Ошибок нет.
pub fn sub_message(id: u64, symbol: &str) -> String {
    json!({"id": id, "method": "orderbook.subscribe", "params": [symbol]}).to_string()
}

/// Решение 427: подписка на полный стакан пары («Subscribe full orderBook»: params ["<symbol>", true], «full depth and
/// interval = 100ms») при full, иначе — sub_message. Принимает: id, символ, полный ли. Ошибок нет.
pub fn sub_message_lane(id: u64, symbol: &str, full: bool) -> String {
    if full {
        json!({"id": id, "method": "orderbook.subscribe", "params": [symbol, true]}).to_string()
    } else {
        sub_message(id, symbol)
    }
}

/// Сообщение отписки от всех стаканов соединения («Unsubscribe OrderBook»). Принимает: id запроса. Ошибок нет.
pub fn unsub_message(id: u64) -> String {
    json!({"id": id, "method": "orderbook.unsubscribe", "params": []}).to_string()
}

/// Сессия: работает до обрыва. Возвращает причину окончания.
#[allow(clippy::too_many_arguments)]
pub async fn session(
    ctx: &Ctx,
    name: &str,
    assets: &[(Asset, String)],
    symbols: &Symbols,
    books: &Books,
    out: &mpsc::Sender<MarketEvent>,
    cfg: Cfg,
    w: Watch<'_>,
    lane: &Lane,
) -> End {
    let rules = symbols.all();
    let mut pairs: HashMap<&str, Pair> = HashMap::new();
    for (a, s) in assets {
        match rules.get(&a.ticker) {
            Some(r) => {
                let p = Pair {
                    asset: a,
                    price_scale: r.price_scale,
                    qty_scale: r.base_scale,
                };
                pairs.insert(s.as_str(), p);
            }
            None => return End::Config(format!("{s}: нет правил пары в справочнике Phemex")),
        }
    }
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
    let alive: Alive = Default::default();
    // id запроса подписки → символ (ответ с ошибкой — пара недостоверна). id 0 — ping.
    let by_id: HashMap<u64, &str> = assets
        .iter()
        .enumerate()
        .map(|(i, (_, s))| (i as u64 + 1, s.as_str()))
        .collect();
    // Очередь отправки (берётся с конца): (id, Some(символ)) — подписка, (id, None) — отписка всех (решение 202).
    let subs_all = || {
        let mut v: Vec<(u64, Option<&str>)> = by_id.iter().map(|(i, s)| (*i, Some(*s))).collect();
        v.sort_by_key(|(i, _)| std::cmp::Reverse(*i));
        v
    };
    let mut pending = subs_all();
    let unsub_id = assets.len() as u64 + 1;
    let mut local: HashMap<String, LocalBook> = HashMap::new();
    let mut sub_tick = tokio::time::interval(cfg.sub_gap);
    let mut ping = tokio::time::interval_at(Instant::now() + cfg.ping, cfg.ping);
    let ping_msg = json!({"id": 0, "method": "server.ping", "params": []}).to_string();
    let mut last_rx = Instant::now();
    let mut dropped = 0u64;
    // Решение 324: стакан ядру через общий набор канала ядра (venue.book_events_keep).
    let kept = BookOut::new(out, ctx.venue(), &ctx.settings, &ctx.journal);
    loop {
        tokio::select! {
            _ = sub_tick.tick(), if !pending.is_empty() => {
                let Some((id, s)) = pending.pop() else { continue };
                let m = s.map_or_else(|| unsub_message(id), |s| sub_message_lane(id, s, lane.full));
                if let Err(e) = sink.send(Message::Text(m.into())).await {
                    return End::Broken(format!("отправка подписки: {e}"));
                }
            }
            Some(c) = w.cmd.recv() => match c {
                Cmd::Reconnect => return End::Broken("сторож пар: переподключение соединения (решение 202)".into()),
                Cmd::Resub(_) => {
                    if pending.is_empty() {
                        pending = subs_all();
                        pending.push((unsub_id, None));
                    }
                    for (a, _) in assets {
                        w.feed.resubscribed(&a.ticker);
                    }
                }
            },
            _ = ping.tick() => {
                if let Err(e) = sink.send(Message::Text(ping_msg.clone().into())).await {
                    return End::Broken(format!("отправка server.ping: {e}"));
                }
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
                alive.store(Utc::now().timestamp_millis(), Ordering::Relaxed);
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
                let is_book = matches!(&msg, Msg::Book(_) | Msg::Bad { symbol: Some(_), .. });
                if !is_book || matches!(ctx.flag("raw.book_frames"), Ok(true)) {
                    ctx.raw.write(ID, name, &text);
                }
                let f = match msg {
                    Msg::Book(f) => f,
                    Msg::Reply { id, ok: false, text } => {
                        if let Some(p) = id.and_then(|i| by_id.get(&i)).and_then(|s| pairs.get(s)) {
                            lane.lose(ctx, books, out, &p.asset.ticker, &format!("{name}: отказ подписки")).await;
                        }
                        ctx.log(Level::Error, "venue.ws", format!("WebSocket {name}: ошибка Phemex: {text}")).await;
                        continue;
                    }
                    Msg::Bad { symbol, reason } => {
                        if let Some((s, p)) = symbol.as_deref().and_then(|s| pairs.get(s).map(|p| (s, p))) {
                            w.feed.frame(&p.asset.ticker);
                            // Решение 427: изменение потеряно — стакан пары этого потока ждёт снимок.
                            local.remove(s);
                            lane.lose(ctx, books, out, &p.asset.ticker, &format!("{name}: кадр не разобран")).await;
                        }
                        ctx.log(Level::Error, "venue.ws", format!("WebSocket {name}: кадр не разобран: {reason}")).await;
                        continue;
                    }
                    _ => continue,
                };
                let Some(p) = pairs.get(f.symbol.as_str()) else { continue };
                w.feed.frame(&p.asset.ticker);
                let lb = local.entry(f.symbol.clone()).or_default();
                match lb.apply(&f, cfg.strict) {
                    Applied::Changed => {}
                    Applied::Differs => book_lane::snap_differs(ctx, books, &p.asset.ticker, name).await,
                    Applied::Waiting => continue,
                    Applied::Gap(why) => {
                        let m = format!("{name}: {why}; поток ждёт снимок (≤ 60 с)");
                        lane.lose(ctx, books, out, &p.asset.ticker, &m).await;
                        if !lane.pick.reserve() {
                            // Ревью 427: без запасного потока журнал как до решения 427 — каждый разрыв.
                            let m = format!("WebSocket {name}: {} — {why}; стакан недостоверен до снимка (≤ 60 с)", f.symbol);
                            ctx.log(Level::Warn, "venue.ws", m).await;
                        }
                        continue;
                    }
                }
                let (bids, asks) = lb.top(cfg.levels, p.price_scale, p.qty_scale);
                let book = OrderBook {
                    venue: ctx.venue(),
                    asset: p.asset.clone(),
                    bids,
                    asks,
                    exchange_time: f.ts_ns.map(|ns| Utc.timestamp_nanos(ns)),
                    received_at: Utc::now(),
                };
                // Ревью 427: память и ядро — под замком пары (порядок двух соединений пары).
                let (fed, ev) = lane.take(&p.asset.ticker, book, f.ts_ns, &alive, |b| {
                    books.put(b.clone(), alive.clone());
                    kept.book(out, b)
                });
                lane.events(ctx, books, &ev, &p.asset.ticker, "кадр стакана").await;
                let Some(fed) = fed else { continue };
                match fed {
                    Fed::Sent | Fed::Kept => {}
                    Fed::Closed => return End::Done,
                    Fed::Lost => dropped += 1,
                }
                if dropped > 0 && dropped.is_power_of_two() {
                    tracing::warn!(conn = name, dropped, "ядро не успевает принимать стаканы Phemex");
                }
            }
        }
    }
}

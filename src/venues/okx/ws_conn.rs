//! Файл: venues/okx/ws_conn.rs
//! Что делает: одна сессия публичного WebSocket стаканов OKX: подключение, подписка всех пар соединения запросами не
//! длиннее 64 КБ, свой «ping» после venue.ws_ping_ms без кадров, тишина дольше venue.ws_silence_ms — обрыв; кадры books5
//! — снимок в память и ядру; кадры books — локальный стакан (local_book.rs), разрыв prevSeqId — пара недостоверна и
//! пересподписка этой пары (изменения до её нового снимка пропускаются); второй разрыв той же пары раньше
//! venue.ws_reconnect_pause_ms или исчерпан часовой предел запросов соединения — переподключение (урок 12: без
//! бесконечного круга ресинков). Отказ подписки (event error с
//! instId) — стакан пары недостоверен. Сырые кадры стакана — в журнал только при raw.book_frames = да (решение 109).
//! Кто вызывает: ws.rs.
//! От чего зависит: depth.rs, local_book.rs, book.rs, ws.rs (Cfg, End), ws_req.rs (подключение, подписки), ctx.rs,
//! gateway/feed_reg.rs (решение 202).
//! Настройки: из ws.rs (Cfg); raw.book_frames [okx].
//!
//! Решение 202 (для всех бирж, Тигран 08.11): каждый кадр отмечается в учёте свежести (gateway/feed_reg.rs) — кадр
//! стакана по паре (снимок books5, изменение и пустой кадр «живо» books, кадр с разрывом, неразборчивый с instId);
//! seqId кадра не передаётся (решение 213: связь seqId кадра с seqId ответа GET /api/v5/market/books в документации не
//! описана — подтверждение тихой пары по содержимому, book_print.rs). Команды сторожа ядра: «переподписать пару» —
//! {"op":"unsubscribe","args":[{"channel","instId"}]}, затем {"op":"subscribe",…} в этом же соединении («Overview →
//! WebSocket → Unsubscribe»: «{"op": "unsubscribe", "args": ["<SubscriptionTopic>"]}», ответ {"event":"unsubscribe",
//! "arg":{…},"connId"}) — тот же путь, что пересподписка пары при разрыве prevSeqId: изменения books до нового снимка
//! пропускаются, соседи не трогаются; оба запроса — в счёт «480 times per hour» (не хватает — переподключение).
//! «Переподключить соединение» — сессия заканчивается, ws.rs подключается заново с теми же парами.
//!
//! Официально (https://www.okx.com/docs-v5/en/, «Overview → WebSocket»): подписка {"op":"subscribe","args":[{"channel",
//! "instId"}, …]}; «the total length of multiple channels cannot exceed 64 KB»; «The total number of 'subscribe'/
//! 'unsubscribe'/'login' requests per connection is limited to 480 times per hour»; уведомление 64008 «The connection
//! will soon be closed for a service upgrade. Please reconnect.»

use std::collections::HashMap;
use std::sync::atomic::Ordering;

use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc::{self, error::TrySendError};
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::Message;

use crate::core::types::{Asset, OrderBook};
use crate::gateway::events::MarketEvent;
use crate::storage::journal::Level;

use super::book::{Alive, Books};
use super::ctx::{Ctx, ID};
use super::depth::{self, Msg};
use super::local_book::{Applied, LocalBook};
use super::ws::{Cfg, End, invalidate};
use super::ws_req::{MAX_OPS_PER_HOUR, open, requests, resub};
use crate::gateway::feed_health::Cmd;
use crate::gateway::feed_reg::ConnHandle;

/// Учёт свежести соединения и канал команд сторожа (решение 202).
pub struct Watch<'a> {
    pub feed: &'a ConnHandle,
    pub cmd: &'a mut mpsc::UnboundedReceiver<Cmd>,
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
    let ws = match open(&cfg.url, cfg.connect_timeout).await {
        Ok(ws) => ws,
        Err(e) => return End::Broken(e),
    };
    let (mut sink, mut stream) = ws.split();
    w.feed.session_started();
    let by_inst: HashMap<&str, &Asset> = assets.iter().map(|(a, s)| (s.as_str(), a)).collect();
    let insts: Vec<&str> = assets.iter().map(|(_, s)| s.as_str()).collect();
    let mut ops = 0usize;
    for r in requests("subscribe", cfg.channel, &insts) {
        ops += 1;
        if let Err(e) = sink.send(Message::Text(r.into())).await {
            return End::Broken(format!("отправка подписки: {e}"));
        }
    }
    let alive: Alive = Default::default();
    let mut local: HashMap<String, LocalBook> = HashMap::new();
    let mut last_gap: HashMap<String, Instant> = HashMap::new();
    // Пары после пересподписки: изменения до нового снимка (ещё летящие по старой подписке) пропускаются.
    let mut resync: std::collections::HashSet<String> = Default::default();
    let mut last_rx = Instant::now();
    let mut pinged = false;
    let mut dropped = 0u64;
    loop {
        tokio::select! {
            _ = tokio::time::sleep_until(last_rx + cfg.ping), if !pinged => {
                pinged = true;
                if let Err(e) = sink.send(Message::Text("ping".into())).await {
                    return End::Broken(format!("отправка ping: {e}"));
                }
            }
            Some(c) = w.cmd.recv() => match c {
                Cmd::Reconnect => return End::Broken("сторож пар: переподключение соединения (решение 202)".into()),
                Cmd::Resub(ticker) => {
                    let Some((_, inst)) = assets.iter().find(|(a, _)| a.ticker == ticker) else { continue };
                    if ops + 2 > MAX_OPS_PER_HOUR {
                        return End::Broken(format!("{inst}: переподписка сторожем, а предел {MAX_OPS_PER_HOUR} запросов исчерпан"));
                    }
                    // Изменения books до нового снимка — по старой подписке, пропускаются (как при разрыве).
                    if cfg.channel == "books" {
                        resync.insert(inst.clone());
                    }
                    if let Err(e) = resub(&mut sink, cfg.channel, inst, &mut ops).await {
                        return End::Broken(format!("{e} (сторож пар)"));
                    }
                    w.feed.resubscribed(&ticker);
                }
            },
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
                pinged = false;
                alive.store(chrono::Utc::now().timestamp_millis(), Ordering::Relaxed);
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
                let is_book = matches!(&msg, Msg::Depth(_) | Msg::Bad { inst: Some(_), .. });
                if !is_book || matches!(ctx.flag("raw.book_frames"), Ok(true)) {
                    ctx.raw.write(ID, name, &text);
                }
                let d = match msg {
                    Msg::Depth(d) => d,
                    Msg::Bad { inst, reason } => {
                        if let Some(a) = inst.as_deref().and_then(|s| by_inst.get(s)) {
                            w.feed.frame(&a.ticker);
                            invalidate(ctx, books, out, &a.ticker);
                        }
                        ctx.log(Level::Error, "venue.ws", format!("WebSocket {name}: кадр не разобран: {reason}")).await;
                        continue;
                    }
                    Msg::Event { event, code, inst, text, .. } => {
                        if code == "64008" {
                            return End::Broken(format!("OKX предупредил о закрытии (64008): {text}"));
                        }
                        if event == "error" {
                            if let Some(a) = inst.as_deref().and_then(|s| by_inst.get(s)) {
                                invalidate(ctx, books, out, &a.ticker);
                            }
                            ctx.log(Level::Error, "venue.ws", format!("WebSocket {name}: ошибка OKX: {text}")).await;
                        }
                        continue;
                    }
                    _ => continue,
                };
                let Some(asset) = by_inst.get(d.inst.as_str()) else { continue };
                w.feed.frame(&asset.ticker);
                if !resync.is_empty() && resync.contains(&d.inst) {
                    if !d.snapshot {
                        continue;
                    }
                    resync.remove(&d.inst);
                }
                let (bids, asks) = if d.channel == "books" {
                    let lb = local.entry(d.inst.clone()).or_default();
                    match lb.apply(&d) {
                        Applied::Changed => lb.top(cfg.levels),
                        Applied::Repeat => {
                            books.touch(&asset.ticker);
                            continue;
                        }
                        Applied::Gap(why) => {
                            invalidate(ctx, books, out, &asset.ticker);
                            let again = last_gap.get(&d.inst).is_some_and(|t| t.elapsed() < cfg.resync_gap);
                            if again || ops + 2 > MAX_OPS_PER_HOUR {
                                return End::Broken(format!("{}: повторный разрыв стакана ({why})", d.inst));
                            }
                            last_gap.insert(d.inst.clone(), Instant::now());
                            resync.insert(d.inst.clone());
                            ctx.log(Level::Warn, "venue.ws", format!("WebSocket {name}: {} — {why}; пересподписка пары", d.inst)).await;
                            if let Err(e) = resub(&mut sink, cfg.channel, &d.inst, &mut ops).await {
                                return End::Broken(e);
                            }
                            w.feed.resubscribed(&asset.ticker);
                            continue;
                        }
                    }
                } else {
                    let keep = |v: Vec<crate::core::types::PriceLevel>| v.into_iter().filter(|l| !l.qty.is_zero()).collect();
                    (keep(d.bids), keep(d.asks))
                };
                let book = OrderBook {
                    venue: ctx.venue(),
                    asset: (*asset).clone(),
                    bids,
                    asks,
                    exchange_time: d.exchange_time,
                    received_at: chrono::Utc::now(),
                };
                books.put(book.clone(), alive.clone());
                match out.try_send(MarketEvent::Book(book)) {
                    Ok(()) => {}
                    Err(TrySendError::Closed(_)) => return End::Done,
                    Err(TrySendError::Full(_)) => dropped += 1,
                }
                if dropped > 0 && dropped.is_power_of_two() {
                    tracing::warn!(conn = name, dropped, "ядро не успевает принимать стаканы OKX");
                }
            }
        }
    }
}

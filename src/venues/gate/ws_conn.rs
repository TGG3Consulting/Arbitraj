//! Файл: venues/gate/ws_conn.rs
//! Что делает: одна сессия WebSocket рынка Gate: подключение, подписки по одной с паузой (venue.ws_subs_per_sec), свой
//! spot.ping раз в venue.ws_ping_ms, Pong на Ping-кадр сервера, запись сырых кадров, стаканы → память и ядру; отказ
//! подписки или неразборчивый стакан → стакан этой пары недостоверен; spot.system upgrade → переподключение. Ядру
//! стаканы отдаются без ожидания (try_send).
//! Решение 324: стакан ядру — gateway/core_out_book.rs (канал полон: venue.book_events_keep = да — последний кадр
//! пары ждёт места в общем наборе канала ядра; не задана — прежний сброс и счёт dropped).
//! Решение 421 (быстрый стакан со страховкой): при venue.obu_level у каждой пары две подписки — снимки spot.order_book
//! (страховка, первой) и поток spot.obu; какой стакан отдавать ядру, решает fast_pick.rs (быстрый, пока синхронизирован;
//! разрыв — сразу снимок и пересинхронизация: unsubscribe + subscribe потока в общей очереди подписок с тем же темпом
//! venue.ws_subs_per_sec), исполняет fast_io.rs. Не задана — только снимки, как раньше.
//! Кто вызывает: ws.rs.
//! От чего зависит: depth.rs, obu.rs, ws_plan.rs (очередь подписок), ws_snap.rs (кадры снимков), fast_pick.rs,
//! fast_io.rs, book.rs, ctx.rs, gateway/tap.rs, tokio-tungstenite.
//! Настройки: приходят из ws.rs (Cfg); raw.book_frames [gate] — писать ли кадры стаканов (обоих каналов) в сырой журнал.
//!
//! Решение 109 (как MEXC): снимок с тем же lastUpdateId, что прежний, ядру не отдаётся — стакан не менялся («Order book
//! ID, which is updated whenever the order book is changed», модель OrderBook REST; в канале — lastUpdateId); меньший
//! — устаревший, пропускается. Время кадра пары обновляется в обоих случаях (book.rs, touch; решение 118, А-🟠8).
//! Сравнение по номеру, а не по уровням: без копий уровней на каждый кадр (решения 130, 131).
//! Решение 125: каждый кадр стакана spot.order_book монеты с открытым окном — в приёмник окна как есть (текст).
//! Решение 202: каждый кадр снимков отмечается в учёте свежести (gateway/feed_health.rs); команды сторожа ядра:
//! «переподписать пару» — unsubscribe + subscribe снимков этой пары (тот же id = номер пары; Spot WebSocket v4: event
//! subscribe | unsubscribe, https://www.gate.com/docs/developers/apiv4/ws/en/) и пересинхронизация её потока spot.obu,
//! соседи не трогаются; «переподключить соединение» — сессия заканчивается, ws.rs подключается заново с теми же парами.
//! Решение 213: в учёт свежести идёт lastUpdateId кадра снимков (на каждом кадре, и с тем же номером) — ядро сравнивает
//! его с id ответа GET /spot/order_book?with_id=true (book_version.rs). Номера spot.obu в учёт не идут: их шкала с id
//! REST в документации не связана (решение 421, вопрос в docs/TASKS.md); поток снимков работает и при быстром стакане.

use std::collections::HashMap;
use std::sync::atomic::Ordering;

use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::Message;

use crate::core::types::Asset;
use crate::gateway::core_out_book::{BookOut, Fed};
use crate::gateway::events::MarketEvent;
use crate::gateway::tap::now_ns;

use super::book::{Alive, Books};
use super::ctx::{Ctx, ID};
use super::depth::{self, BOOK_CHANNEL, Msg};
use super::fast_io::Env;
use super::fast_pick::Fast;
use super::obu::{self, Obu};
use super::ws::{Cfg, End};
use super::ws_plan::Plan;
use super::ws_snap::Snaps;
use crate::gateway::feed_health::Cmd;
use crate::gateway::feed_reg::ConnHandle;

/// Учёт свежести соединения и канал команд сторожа (решение 202).
pub struct Watch<'a> {
    pub feed: &'a ConnHandle,
    pub cmd: &'a mut mpsc::UnboundedReceiver<Cmd>,
}

/// Канал стакана для показа на странице окна: «spot.order_book CAKE_USDT 20 100ms». Ошибок нет.
pub fn channel(symbol: &str, levels: u64, interval: &str) -> String {
    format!("{BOOK_CHANNEL} {symbol} {levels} {interval}")
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
    let alive: Alive = Default::default();
    let by_symbol: HashMap<&str, &Asset> = assets.iter().map(|(a, s)| (s.as_str(), a)).collect();
    // Решение 421: быстрые стаканы соединения и очередь подписок (ws_plan.rs).
    let mut fast = Fast::new(cfg.obu, cfg.levels);
    let mut plan = Plan::new(assets, &cfg);
    let mut sub_tick = tokio::time::interval(cfg.sub_gap);
    let mut ping = tokio::time::interval(cfg.ping);
    let mut last_rx = Instant::now();
    let mut dropped = 0u64;
    // Решение 324: стакан ядру через общий набор канала ядра (venue.book_events_keep).
    let kept = BookOut::new(out, ctx.venue(), &ctx.settings, &ctx.journal);
    let env = Env {
        ctx,
        name,
        books,
        out,
        kept: &kept,
        alive: &alive,
        by_symbol: &by_symbol,
        assets,
    };
    // Кадры снимков spot.order_book: последний отданный ядру номер по символу (решение 109) — ws_snap.rs.
    let mut snaps = Snaps::default();
    loop {
        tokio::select! {
            _ = sub_tick.tick(), if plan.busy(&fast) => {
                let reqs = plan.step(&mut fast);
                for req in reqs {
                    if let Err(e) = sink.send(Message::Text(req.into())).await {
                        return End::Broken(format!("отправка подписки: {e}"));
                    }
                }
            }
            Some(c) = w.cmd.recv() => match c {
                Cmd::Reconnect => return End::Broken("сторож пар: переподключение соединения (решение 202)".into()),
                Cmd::Resub(ticker) => {
                    let Some(i) = assets.iter().position(|(a, _)| a.ticker == ticker) else { continue };
                    for ev in ["unsubscribe", "subscribe"] {
                        let req = plan.book_req(ev, i);
                        if let Err(e) = sink.send(Message::Text(req.into())).await {
                            return End::Broken(format!("отправка {ev} (сторож пар): {e}"));
                        }
                    }
                    snaps.last.remove(&assets[i].1);
                    fast.resync(&assets[i].1);
                    w.feed.resubscribed(&ticker);
                }
            },
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
                alive.store(chrono::Utc::now().timestamp_millis(), Ordering::Relaxed);
                w.feed.alive();
                let t = match frame {
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
                let raw_books = matches!(ctx.flag("raw.book_frames"), Ok(true));
                let fed = if let Some(m) = obu::parse(&t) {
                    if matches!(m, Obu::Ack { .. }) || raw_books {
                        ctx.raw.write(ID, name, &t);
                    }
                    env.on_obu(&mut fast, m).await
                } else {
                    let m = depth::parse(&t);
                    if !matches!(m, Msg::Depth { .. } | Msg::Bad { .. }) || raw_books {
                        ctx.raw.write(ID, name, &t);
                    }
                    if matches!(m, Msg::Upgrade) {
                        return End::Broken("spot.system: сервер обновляется (upgrade)".into());
                    }
                    let Some(fed) = snaps.on_msg(&env, &mut fast, w.feed, m, &t, rx_ns).await else { continue };
                    fed
                };
                match fed {
                    Fed::Sent | Fed::Kept => {}
                    Fed::Closed => return End::Done,
                    Fed::Lost => dropped += 1,
                }
                if dropped > 0 && dropped.is_power_of_two() {
                    tracing::warn!(conn = name, dropped, "ядро не успевает принимать стаканы Gate");
                }
            }
        }
    }
}

//! Файл: venues/mexc/ws_conn.rs
//! Что делает: одна сессия WebSocket рынка MEXC: подключение, подписки по одной с паузой ({"method":"SUBSCRIPTION"}),
//! свой {"method":"PING"} раз в venue.ws_ping_ms, ответ Pong на Ping-кадр сервера, запись сырых кадров, стаканы
//! (protobuf) → память и ядру, отказ подписки или неразборчивый стакан → стакан этой пары недостоверен.
//! Ядру стаканы отдаются без ожидания (try_send).
//! Решение 324: стакан ядру — gateway/core_out_book.rs (канал полон: venue.book_events_keep = да — последний кадр
//! пары ждёт места в общем наборе канала ядра; не задана — прежний сброс и счёт dropped).
//! Кто вызывает: ws.rs.
//! От чего зависит: depth.rs, book.rs, ctx.rs, ws_sess.rs, gateway/feed_reg.rs, tokio-tungstenite.
//! Настройки: приходят из ws.rs (Cfg).
//!
//! Тишина считается от последнего кадра (как решение 98 у bsc_dex): любой кадр, включая PONG, — соединение живо.
//! Сырой журнал: текстовые кадры — как есть; двоичные (стаканы protobuf) — hex, только если raw.book_frames [mexc] = да
//! (решение 109: снимки стаканов MEXC идут потоком — запись каждого кадра нагружала диск и процессор).
//! Решение 109: снимок стакана, не отличающийся от прежнего (те же уровни), ядру не отдаётся — частичный стакан MEXC
//! присылается снимками и без изменений; свежесть — по соединению (alive) и по времени кадра пары (book.rs, touch —
//! и для пропущенного одинакового снимка; решение 118, А-🟠8).
//! Решение 202 (для всех бирж, 08.11): каждый кадр отмечается в учёте свежести (gateway/feed_reg.rs) — кадр стакана по
//! паре, и одинаковый снимок тоже; версия книги не передаётся (решение 213: поле version частичного стакана в
//! документации — «Version number», без связи с lastUpdateId REST /api/v3/depth); тихая пара подтверждается по
//! содержимому — стакан в памяти против ответа REST (book_print.rs, Тигран 08.11).
//! Команды сторожа ядра: «переподписать пару» — {"method":"UNSUBSCRIPTION","params":[канал]}, затем
//! {"method":"SUBSCRIPTION","params":[канал]} в этом же соединении (Live Subscription/Unsubscription to Data Streams,
//! https://www.mexc.com/api-docs/spot-v3/websocket-market-streams/live-subscriptionunsubscription-to-data-streams; тот же
//! вид отписки — feed.rs), соседи не трогаются; «переподключить соединение» — сессия заканчивается, ws.rs подключается
//! заново с теми же парами.
//! Решение 420 (быстрый стакан, задана venue.fast_book_interval): на каждую пару — ещё канал изменений
//! spot@public.aggre.depth.v3.api.pb@<интервал>@<SYMBOL> в этом же соединении (2 подписки на пару, предел 30 — пар не
//! больше 15, market.rs); состояние сессии и разбор кадров — ws_sess.rs; какой стакан ядру и страховка снимками —
//! fast_pair.rs; отдача ядру — ws_out.rs. Настройки очереди снимков не заданы или вне пределов, пар больше 15 —
//! быстрый стакан в этой сессии выключен (журнал), только снимки. Переподписка сторожем — оба канала пары, быстрый
//! стакан пары заново со снимка.

use std::collections::VecDeque;
use std::sync::atomic::Ordering;

use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::Message;

use crate::core::types::Asset;
use crate::gateway::events::MarketEvent;
use crate::gateway::tap::{TapData, TapStream, now_ns};

use super::book::{Alive, Books};
use super::ctx::{Ctx, ID};
use super::depth::{self, Msg};
use super::ws::{Cfg, End};
use super::ws_sess::Sess;
use crate::gateway::feed_health::Cmd;
use crate::gateway::feed_reg::ConnHandle;

/// Учёт свежести соединения и канал команд сторожа (решение 202).
pub struct Watch<'a> {
    pub feed: &'a ConnHandle,
    pub cmd: &'a mut mpsc::UnboundedReceiver<Cmd>,
}

/// Канал частичного стакана: «spot@public.limit.depth.v3.api.pb@<symbol>@<level>» (официальная документация).
pub fn channel(symbol: &str, levels: u64) -> String {
    format!("spot@public.limit.depth.v3.api.pb@{symbol}@{levels}")
}

/// Решение 420: канал изменений «spot@public.aggre.depth.v3.api.pb@(100ms|10ms)@<symbol>» (Diff.Depth Stream,
/// https://www.mexc.com/api-docs/spot-v3/websocket-market-streams/diffdepth-stream). Принимает: интервал, символ.
pub fn fast_channel(interval: &str, symbol: &str) -> String {
    format!("spot@public.aggre.depth.v3.api.pb@{interval}@{symbol}")
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
    // t79: пары соединения подписаны в этой сессии — стакан из ответа REST сторожа (book_rest.rs) только с её
    // живостью.
    books.bind(assets, &alive);
    let (mut sess, mut snap_rx) =
        Sess::new(ctx, name, assets, books, out, w.feed, &cfg, alive.clone()).await;
    let mut pending: VecDeque<String> = assets.iter().flat_map(|(_, s)| sess.channels(s)).collect();
    let mut sub_tick = tokio::time::interval(cfg.sub_gap);
    let mut ping = tokio::time::interval(cfg.ping);
    let mut last_rx = Instant::now();
    loop {
        tokio::select! {
            _ = sub_tick.tick(), if !pending.is_empty() => {
                let Some(ch) = pending.pop_front() else { continue };
                let sub = json!({ "method": "SUBSCRIPTION", "params": [ch] });
                if let Err(e) = sink.send(Message::Text(sub.to_string().into())).await {
                    return End::Broken(format!("отправка подписки: {e}"));
                }
            }
            Some(c) = w.cmd.recv() => match c {
                Cmd::Reconnect => return End::Broken("сторож пар: переподключение соединения (решение 202)".into()),
                Cmd::Resub(ticker) => {
                    let Some((_, sym)) = assets.iter().find(|(a, _)| a.ticker == ticker) else { continue };
                    for ch in sess.channels(sym) {
                        for method in ["UNSUBSCRIPTION", "SUBSCRIPTION"] {
                            let m = json!({ "method": method, "params": [ch] });
                            if let Err(e) = sink.send(Message::Text(m.to_string().into())).await {
                                return End::Broken(format!("отправка {method} (сторож пар): {e}"));
                            }
                        }
                    }
                    // Первый снимок после переподписки — ядру, даже если такой же (решение 109); быстрый стакан —
                    // заново (решение 420).
                    if !sess.resubscribed(&ticker) {
                        return End::Done;
                    }
                    w.feed.resubscribed(&ticker);
                }
            },
            Some(note) = snap_rx.recv() => {
                if !sess.on_note(note) {
                    return End::Done;
                }
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
                alive.store(chrono::Utc::now().timestamp_millis(), Ordering::Relaxed);
                w.feed.alive();
                let msg = match frame {
                    Message::Binary(b) => {
                        if matches!(ctx.flag("raw.book_frames"), Ok(true)) {
                            ctx.raw.write(ID, name, &hex::encode(&b));
                        }
                        let m = depth::parse_binary(&b);
                        if let Msg::Depth { symbol, .. } | Msg::Bad { symbol: Some(symbol), .. } = &m
                            && let Some(a) = sess.asset(symbol)
                        {
                            ctx.taps.push(ID, &a.ticker, TapStream::Book, name, rx_ns, || TapData::Binary(b.to_vec()));
                        }
                        m
                    }
                    Message::Text(t) => {
                        ctx.raw.write(ID, name, &t);
                        depth::parse_text(&t)
                    }
                    Message::Ping(p) => {
                        if let Err(e) = sink.send(Message::Pong(p)).await {
                            return End::Broken(format!("отправка Pong: {e}"));
                        }
                        continue;
                    }
                    Message::Close(c) => return End::Broken(format!("сервер закрыл соединение: {c:?}")),
                    _ => continue,
                };
                if !sess.on_msg(msg).await {
                    return End::Done;
                }
                let dropped = sess.give.dropped;
                if dropped > 0 && pending.is_empty() && dropped.is_power_of_two() {
                    tracing::warn!(conn = name, dropped, "ядро не успевает принимать стаканы MEXC");
                }
            }
        }
    }
}

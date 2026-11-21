//! Файл: venues/htx/fast_conn.rs
//! Что делает: решение 422 — одна сессия WebSocket быстрого канала стакана HTX (адрес /feed): подключение, подписки
//! market.$symbol.mbp.$levels по одной с паузой venue.ws_subs_per_sec, запросы снимка {"req"} по одному с паузой
//! venue.fast_book_req_per_sec (официально «The limitation of single connection is 100 ms»), {"pong": n} на каждый
//! {"ping": n}, тишина дольше venue.ws_silence_ms — обрыв; кадры GZIP. Ответа на запрос снимка нет дольше
//! http.timeout_ms — новый запрос (проверка на каждом ping сервера; решение 422, ревью). Изменения и снимки — fast_pair.rs.
//! Канал 5/20 уровней при тишине рынка молчит («won't disseminate any update in such a case») — тишина пары не
//! ошибка: свежесть тихой пары подтверждают снимки mbp.refresh (book_mode.rs) и сторож ядра (решение 213,
//! book_print.rs); тишину всего соединения ловит venue.ws_silence_ms (ping сервера ~5 с идёт всегда).
//! Сырые кадры стакана — в журнал только при raw.book_frames = да (как у /ws, решение 109).
//! Кто вызывает: fast_ws.rs.
//! От чего зависит: fast_pair.rs, fast_reqs.rs, mbp.rs, depth.rs (gunzip, Msg), book.rs, ctx.rs, ws.rs (End),
//! tokio-tungstenite.
//! Настройки: из fast_ws.rs (FastCfg; срок ответа на req — http.timeout_ms, connect_timeout); raw.book_frames [htx].
//!
//! Официально (https://huobiapi.github.io/docs/spot/v1/en/, «Websocket Market Data»): GZIP, ping/pong и подписка — как
//! у /ws («Introduction», «Subscribe to Topic»); ошибки — {"status": "error", "err-code", "err-msg", "id"} (в т.ч.
//! «bad-request | 429 too many request», «invalid topic»). Свой id подписки — символ, запроса снимка — «req.» + символ.

use std::collections::HashMap;
use std::sync::atomic::Ordering;

use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::Message;

use crate::core::types::Asset;
use crate::gateway::core_out_book::BookOut;
use crate::gateway::events::MarketEvent;
use crate::storage::journal::Level;

use super::book::{Alive, Books};
use super::ctx::{Ctx, ID};
use super::depth::{self, Msg};
use super::fast_book::Update;
use super::fast_pair::Conn;
use super::fast_ws::FastCfg;
use super::mbp::{self, Feed};
use super::ws::End;

/// Сессия: работает до обрыва. Принимает: окружение, имя соединения, монеты (монета, символ HTX), стаканы, канал
/// ядра, настройки, канал пересинхронизации (book_mode.rs resync). Возвращает: причину окончания.
pub async fn session(
    ctx: &Ctx,
    name: &str,
    assets: &[(Asset, String)],
    books: &Books,
    out: &mpsc::Sender<MarketEvent>,
    cfg: &FastCfg,
    resync: &mut mpsc::UnboundedReceiver<String>,
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
    // Просьбы прошлой сессии не нужны: новая начинает каждую пару со снимка.
    while resync.try_recv().is_ok() {}
    let alive: Alive = Default::default();
    let mut c = Conn {
        ctx,
        books,
        out,
        kept: BookOut::new(out, ctx.venue(), &ctx.settings, &ctx.journal),
        alive: alive.clone(),
        cfg,
        by_symbol: assets.iter().map(|(a, s)| (s.as_str(), a)).collect(),
        local: HashMap::new(),
        reqs: Default::default(),
    };
    let mut pending: Vec<String> = assets
        .iter()
        .rev()
        .map(|(_, s)| mbp::sub_message(s, cfg.levels))
        .collect();
    let mut sub_tick = tokio::time::interval(cfg.sub_gap);
    let mut req_tick = tokio::time::interval(cfg.req_gap);
    let mut last_rx = Instant::now();
    loop {
        tokio::select! {
            _ = sub_tick.tick(), if !pending.is_empty() => {
                let Some(m) = pending.pop() else { continue };
                if let Err(e) = sink.send(Message::Text(m.into())).await {
                    return End::Broken(format!("отправка подписки: {e}"));
                }
            }
            _ = req_tick.tick(), if !c.reqs.is_empty() => {
                let Some(s) = c.reqs.due() else { continue };
                if let Err(e) = sink.send(Message::Text(mbp::req_message(&s, cfg.levels).into())).await {
                    return End::Broken(format!("отправка запроса снимка: {e}"));
                }
            }
            Some(s) = resync.recv() => {
                if !c.on_resync(&s).await {
                    return End::Done;
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
                alive.store(chrono::Utc::now().timestamp_millis(), Ordering::Relaxed);
                let text = match frame {
                    Message::Binary(b) => match depth::gunzip(&b) {
                        Ok(t) => t,
                        Err(e) => {
                            ctx.log(Level::Error, "venue.ws", format!("WebSocket {name}: {e}")).await;
                            continue;
                        }
                    },
                    Message::Text(t) => t.to_string(),
                    Message::Ping(p) => {
                        if let Err(e) = sink.send(Message::Pong(p)).await {
                            return End::Broken(format!("отправка Pong: {e}"));
                        }
                        continue;
                    }
                    Message::Close(f) => return End::Broken(format!("сервер закрыл соединение: {f:?}")),
                    _ => continue,
                };
                let feed = mbp::parse(&text);
                let is_book = matches!(&feed, Feed::Update { .. } | Feed::Refresh { .. } | Feed::Bad { symbol: Some(_), .. });
                if !is_book || matches!(ctx.flag("raw.book_frames"), Ok(true)) {
                    ctx.raw.write(ID, name, &text);
                }
                let open = match feed {
                    Feed::Service(Msg::Ping(n)) => {
                        let pong = json!({ "pong": n }).to_string();
                        if let Err(e) = sink.send(Message::Text(pong.into())).await {
                            return End::Broken(format!("отправка pong: {e}"));
                        }
                        // Решение 422, ревью: ответа на req нет дольше http.timeout_ms — новый запрос (проверка на
                        // ping сервера — не на каждый кадр).
                        let mut open = true;
                        for s in c.reqs.late(cfg.connect_timeout) {
                            let why = format!("{s}: нет ответа на запрос снимка дольше http.timeout_ms");
                            open = open && c.broken(&s, why).await;
                        }
                        open
                    }
                    Feed::Service(Msg::Subscribed(topic)) => {
                        // Шаги 1–2 процедуры: изменения уже копятся — запросить снимок того же числа уровней.
                        if let Some(s) = depth::symbol_of(&topic) {
                            c.reqs.queue(s, std::time::Duration::ZERO);
                        }
                        true
                    }
                    Feed::Service(Msg::Error { id: Some(id), text }) if c.by_symbol.contains_key(id.strip_prefix(mbp::REQ_ID).unwrap_or(id.as_str())) => {
                        c.on_error(&id, &text).await
                    }
                    Feed::Service(Msg::Error { text, .. }) => {
                        ctx.log(Level::Error, "venue.ws", format!("WebSocket {name}: ошибка HTX: {text}")).await;
                        true
                    }
                    Feed::Update { symbol, seq, prev, bids, asks, ts } => {
                        c.on_update(&symbol, Update { seq, prev, bids, asks, ts }).await
                    }
                    Feed::Refresh { symbol, seq, bids, asks } => c.on_refresh(&symbol, seq, &bids, &asks).await,
                    Feed::Bad { symbol: Some(s), reason } => {
                        ctx.log(Level::Error, "venue.ws", format!("WebSocket {name}: кадр {s} не разобран: {reason}")).await;
                        c.reqs.forget(&s);
                        c.broken(&s, format!("{s}: кадр не разобран: {reason}")).await
                    }
                    Feed::Bad { symbol: None, reason } => {
                        ctx.log(Level::Error, "venue.ws", format!("WebSocket {name}: кадр не разобран: {reason}")).await;
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

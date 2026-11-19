//! Файл: venues/kucoin/ws_conn.rs
//! Что делает: одна сессия WebSocket рынка KuCoin: подключение по токену, ожидание welcome, подписки темами по ≤ 100
//! символов с паузой, свой ping раз в pingInterval токена, тишина дольше pingInterval + pingTimeout — обрыв; кадры —
//! ws_sess.rs (снимки стакана → память и ядру; отказ подписки или неразборчивый снимок → стакан пары недостоверен).
//! Решение 423: при быстром стакане (ws.rs Cfg.fast) у каждой пачки вторая тема — изменения /market/level2 (id
//! sub{N}f); итоги снимков REST Full OrderBook приходят в канал сессии (ws_full.rs on_full); переподписка сторожем —
//! обе темы пачки, пары пачки на снимках до новой синхронизации (ws_fast.rs resubscribed).
//! Решение 324: стакан ядру — gateway/core_out_book.rs (канал полон: venue.book_events_keep = да — последний кадр
//! пары ждёт места в общем наборе канала ядра; не задана — прежний сброс и счёт dropped).
//! Кто вызывает: ws.rs; open() — feed.rs, user_stream_conn.rs.
//! От чего зависит: depth.rs, book.rs, bullet.rs, ctx.rs, ws_sess.rs, ws_fast.rs, gateway/feed_reg.rs, tokio-tungstenite.
//! Настройки: приходят из ws.rs (Cfg); raw.book_frames [kucoin].
//!
//! Официально (Classic WebSocket introduction): «The connection is usable only after the welcome message arrives»;
//! подписка {"id","type":"subscribe","topic","privateChannel","response"}, несколько символов в теме через запятую;
//! ping — «at least once every pingInterval». Тема снимка — «/spotMarket/level2Depth{5|50}:{symbol},{symbol}», «a topic
//! supports up to 100 symbols» (спецификация SDK spec/ws/openapi-spot-public.json, x-topic). Живая запись подписки и
//! кадров: tests/data/cex_samples_2026_11_04/kucoin_ws.jsonl.
//! Решение 202 (для всех бирж, 08.11): каждый кадр отмечается в учёте свежести (gateway/feed_reg.rs) — кадр стакана по
//! паре (и неразборчивый тоже); номера в кадре Level5/Level50 нет — подтверждение тихой пары по содержимому
//! (book_print.rs). Команды сторожа ядра: «переподписать пару» — {"type":"unsubscribe"} и {"type":"subscribe"} ТОЙ ЖЕ
//! темы, в которой пара подписана (до SYMBOLS_PER_TOPIC пар), в этом же соединении; отписка одной пары из темы с
//! несколькими символами в прочитанной документации (спецификация SDK, запись 04.11) не описана — переподписывается вся
//! тема (вопрос — docs/TASKS.md), соседи по теме тоже получают начало прогрева. Отказ подписки (type error с id темы) —
//! стаканы темы недостоверны, как при подключении. «Переподключить соединение» — сессия заканчивается, ws.rs
//! подключается заново с теми же парами (новый токен).

use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

use crate::core::types::Asset;
use crate::gateway::core_out_book::BookOut;
use crate::gateway::events::MarketEvent;
use crate::gateway::tap::now_ns;

use super::book::{Alive, Books};
use super::bullet::Token;
use super::ctx::Ctx;
use super::depth::{self, Msg};
use super::fast_state::Fast;
use super::ws::{Cfg, End};
use super::ws_sess::{FAST_ID, INCR_TOPIC, Sess};
use crate::gateway::feed_health::Cmd;
use crate::gateway::feed_reg::ConnHandle;

/// Учёт свежести соединения и канал команд сторожа (решение 202).
pub struct Watch<'a> {
    pub feed: &'a ConnHandle,
    pub cmd: &'a mut mpsc::UnboundedReceiver<Cmd>,
}

/// Символов в одной теме — официально «a topic supports up to 100 symbols».
pub const SYMBOLS_PER_TOPIC: usize = 100;
/// Тем на одно соединение Spot — официально «Spot/Margin is limited to 400».
pub const TOPICS_PER_CONN: usize = 400;

/// Открытое соединение.
pub type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// Тема снимка стакана для символов. Ошибок нет.
pub fn topic(levels: u64, symbols: &[&str]) -> String {
    format!("/spotMarket/level2Depth{levels}:{}", symbols.join(","))
}

/// Решение 423: тема изменений стакана для символов — «/market/level2:{symbol},{symbol}», «A topic supports up to 100
/// symbols» (https://www.kucoin.com/docs-new/3470068w0). Ошибок нет.
pub fn incr_topic(symbols: &[&str]) -> String {
    format!("{INCR_TOPIC}{}", symbols.join(","))
}

/// Что делает: подключение по токену и ожидание welcome. Принимает: токен, срок. Ошибка — текст (без токена).
pub async fn open(t: &Token, timeout: Duration) -> Result<Ws, String> {
    let connect =
        tokio::time::timeout(timeout, tokio_tungstenite::connect_async(t.url.as_str())).await;
    let mut ws = match connect {
        Err(_) => return Err("не удалось подключиться за отведённое время".into()),
        Ok(Err(e)) => {
            return Err(format!(
                "подключение: {}",
                e.to_string().replace(t.url.as_str(), "***")
            ));
        }
        Ok(Ok((ws, _))) => ws,
    };
    let welcome = tokio::time::timeout(timeout, async {
        while let Some(Ok(f)) = ws.next().await {
            if let Message::Text(s) = f
                && matches!(depth::parse(&s), Msg::Welcome)
            {
                return true;
            }
        }
        false
    });
    match welcome.await {
        Ok(true) => Ok(ws),
        _ => Err("нет welcome за http.timeout_ms — соединение не готово".into()),
    }
}

/// Сессия: работает до обрыва. Возвращает причину окончания.
#[allow(clippy::too_many_arguments)]
pub async fn session(
    ctx: &Ctx,
    name: &str,
    assets: &[(Asset, String)],
    books: &Books,
    out: &mpsc::Sender<MarketEvent>,
    cfg: Cfg,
    token: Token,
    w: Watch<'_>,
) -> End {
    let ws = match open(&token, cfg.connect_timeout).await {
        Ok(ws) => ws,
        Err(e) => return End::Broken(e),
    };
    let (mut sink, mut stream) = ws.split();
    w.feed.session_started();
    let alive: Alive = Default::default();
    // t79: пары соединения подписаны в этой сессии — стакан из ответа REST сторожа (book_rest.rs) только с её
    // живостью.
    books.bind(assets, &alive);
    let syms: Vec<&str> = assets.iter().map(|(_, s)| s.as_str()).collect();
    let by_id: HashMap<String, Vec<&str>> = syms
        .chunks(SYMBOLS_PER_TOPIC)
        .enumerate()
        .map(|(i, c)| (format!("sub{i}"), c.to_vec()))
        .collect();
    // Подписки пачек: тема снимков (id sub{N}); решение 423 — и тема изменений (id sub{N}f).
    let mut pending: Vec<(String, String)> = Vec::new();
    for i in 0..by_id.len() {
        let id = format!("sub{i}");
        pending.extend(topics(&cfg, &id, &by_id[&id]));
    }
    pending.reverse();
    let mut sub_tick = tokio::time::interval(cfg.sub_gap);
    let mut ping = tokio::time::interval_at(Instant::now() + token.ping, token.ping);
    let silence = token.ping + token.ping_timeout;
    let mut last_rx = Instant::now();
    // Решение 423: итоги снимков REST быстрого стакана — канал на все пары соединения.
    let (snap_tx, mut snap_rx) = mpsc::channel(assets.len().max(1));
    let fast_on = cfg.fast.is_some();
    let mut s = Sess {
        ctx,
        name,
        books,
        out,
        // Решение 324: стакан ядру через общий набор канала ядра (venue.book_events_keep).
        kept: BookOut::new(out, ctx.venue(), &ctx.settings, &ctx.journal),
        alive: alive.clone(),
        feed: w.feed,
        by_symbol: assets.iter().map(|(a, s)| (s.as_str(), a)).collect(),
        by_id: by_id.clone(),
        levels: cfg.levels as usize,
        given: HashMap::new(),
        dropped: 0,
        fast: cfg.fast.map(|pause| Fast::new(snap_tx, pause)),
    };
    loop {
        tokio::select! {
            _ = sub_tick.tick(), if !pending.is_empty() => {
                let Some((id, t)) = pending.pop() else { continue };
                let sub = json!({"id": id, "type": "subscribe", "topic": t, "response": true});
                if let Err(e) = sink.send(Message::Text(sub.to_string().into())).await {
                    return End::Broken(format!("отправка подписки: {e}"));
                }
            }
            Some(c) = w.cmd.recv() => match c {
                Cmd::Reconnect => return End::Broken("сторож пар: переподключение соединения (решение 202)".into()),
                Cmd::Resub(ticker) => {
                    let Some((_, sym)) = assets.iter().find(|(a, _)| a.ticker == ticker) else { continue };
                    let Some((id, chunk)) = by_id.iter().find(|(_, c)| c.contains(&sym.as_str())) else { continue };
                    // Отказ подписки — тем же id темы (стаканы темы недостоверны), отписки — своим id.
                    for (sid, t) in topics(&cfg, id, chunk) {
                        for (op, rid) in [("unsubscribe", format!("{sid}-un")), ("subscribe", sid.clone())] {
                            let m = json!({"id": rid, "type": op, "topic": t, "response": true});
                            if let Err(e) = sink.send(Message::Text(m.to_string().into())).await {
                                return End::Broken(format!("отправка {op} (сторож пар): {e}"));
                            }
                        }
                    }
                    for sy in chunk {
                        if let Some(a) = s.by_symbol.get(sy) {
                            w.feed.resubscribed(&a.ticker);
                        }
                    }
                    if let Some(end) = s.resubscribed(chunk).await {
                        return end;
                    }
                }
            },
            Some(done) = snap_rx.recv(), if fast_on => {
                if let Some(end) = s.on_full(done).await {
                    return end;
                }
            }
            _ = ping.tick() => {
                let p = json!({"id": now_ns().to_string(), "type": "ping"});
                if let Err(e) = sink.send(Message::Text(p.to_string().into())).await {
                    return End::Broken(format!("отправка ping: {e}"));
                }
            }
            _ = tokio::time::sleep_until(last_rx + silence) => {
                return End::Broken(format!("нет кадров дольше pingInterval + pingTimeout ({} мс)", silence.as_millis()));
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
                if let Some(end) = s.on_text(&text, rx_ns).await {
                    return end;
                }
            }
        }
    }
}

/// Темы пачки для подписки: (id, тема) снимков; при быстром стакане (решение 423) — и (id + FAST_ID, тема изменений).
/// Ошибок нет.
fn topics(cfg: &Cfg, id: &str, chunk: &[&str]) -> Vec<(String, String)> {
    let mut v = vec![(id.to_string(), topic(cfg.levels, chunk))];
    if cfg.fast.is_some() {
        v.push((format!("{id}{FAST_ID}"), incr_topic(chunk)));
    }
    v
}

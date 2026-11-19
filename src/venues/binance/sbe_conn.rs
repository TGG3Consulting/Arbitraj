//! Файл: venues/binance/sbe_conn.rs
//! Что делает: решение 425 — одна сессия WebSocket SBE рынка Binance для пачки монет: подключение с ключом
//! (sbe_run.rs connect), SUBSCRIBE потоков <symbol>@depth пачками по venue.ws_streams_per_msg с темпом
//! venue.ws_subs_per_sec, Pong на Ping (тот же payload), двоичные кадры → sbe.rs → пары (sbe_pair.rs), снимки из очереди
//! (sbe_snap.rs), команды «пересинхронизировать пару» от сторожа ядра (fast.rs resync). Текстовые кадры — ответы на
//! подписку (отказ — пары запроса на страховку) и serverShutdown (сессия заканчивается — переподключение).
//! Кто вызывает: sbe_run.rs.
//! От чего зависит: sbe.rs, sbe_pair.rs, sbe_snap.rs, fast.rs, depth.rs (разбор ответов JSON), book.rs (Alive), ctx.rs.
//! Настройки: из sbe_run.rs (SbeCfg); raw.book_frames [binance] — писать ли кадры в сырой журнал (двоичные — hex).
//!
//! Официально (sbe-market-data-streams.md): «You must send subscription requests in JSON in WebSocket text frames. You
//! will receive subscription responses in JSON in WebSocket text frames»; «You will receive serverShutdown events in
//! JSON»; «You will receive market data events in SBE in WebSocket binary frames»; «The server sends a ping frame every
//! 20 seconds … you must send a pong with a copy of ping's payload as soon as possible»; «serverShutdown … Please
//! establish a new connection as soon as possible». Формат SUBSCRIBE и ответа — web-socket-streams.md «Live
//! Subscribing/Unsubscribing to streams» (depth.rs parse).

use std::collections::HashMap;
use std::sync::atomic::Ordering;

use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::Message;

use crate::core::types::Asset;
use crate::storage::journal::Level;

use super::book::Alive;
use super::ctx::{Ctx, ID};
use super::depth::{self, Msg};
use super::fast::Fast;
use super::sbe::{self, Frame};
use super::sbe_pair::Pairs;
use super::sbe_run::{SbeCfg, connect};
use super::ws::End;

/// Сессия: работает до обрыва. Принимает: окружение, имя, монеты (монета, символ), арбитр, настройки.
/// Возвращает: причину окончания.
pub async fn session(
    ctx: &Ctx,
    name: &str,
    assets: &[(Asset, String)],
    fast: &Fast,
    cfg: SbeCfg,
) -> End {
    let c = &cfg.base;
    let ws = match connect(&cfg.url, &cfg.key, c.connect_timeout).await {
        Ok(ws) => ws,
        Err(why) => return End::Broken(why),
    };
    let (mut sink, mut stream) = ws.split();
    let alive: Alive = Default::default();
    let (snap_tx, mut snap_rx) = mpsc::channel(assets.len().max(1));
    let (resync_tx, mut resync_rx) = mpsc::unbounded_channel();
    fast.set_resync(Some(resync_tx));
    let mut pairs = Pairs::new(
        ctx,
        fast,
        assets,
        c.levels as usize,
        cfg.resync_pause,
        snap_tx,
        alive.clone(),
    );
    let names: Vec<String> = assets
        .iter()
        .map(|(_, s)| format!("{}@depth", s.to_lowercase()))
        .collect();
    let mut batches: Vec<Vec<String>> = names.chunks(c.per_msg).map(<[_]>::to_vec).collect();
    batches.reverse();
    let mut sent: HashMap<String, Vec<String>> = HashMap::new();
    let mut next_id = 0u64;
    let mut sub_tick = tokio::time::interval(c.sub_gap);
    let mut last_rx = Instant::now();
    loop {
        tokio::select! {
            // Снимки первыми: Start должен начать копить изменения раньше кадров, пришедших после запроса.
            biased;
            Some(done) = snap_rx.recv() => {
                if !pairs.on_done(done) {
                    return End::Done;
                }
            }
            Some(ticker) = resync_rx.recv() => pairs.resync(&ticker),
            _ = sub_tick.tick(), if !batches.is_empty() => {
                let Some(params) = batches.pop() else { continue };
                next_id += 1;
                let sub = json!({ "method": "SUBSCRIBE", "params": params, "id": next_id });
                if let Err(e) = sink.send(Message::Text(sub.to_string().into())).await {
                    return End::Broken(format!("отправка SUBSCRIBE: {e}"));
                }
                sent.insert(next_id.to_string(), params);
            }
            _ = tokio::time::sleep_until(last_rx + c.silence) => {
                return End::Broken(format!("нет кадров дольше {} мс", c.silence.as_millis()));
            }
            next = stream.next() => {
                let frame = match next {
                    None => return End::Broken("соединение закрыто сервером".into()),
                    Some(Err(e)) => return End::Broken(format!("ошибка чтения: {e}")),
                    Some(Ok(f)) => f,
                };
                last_rx = Instant::now();
                alive.store(chrono::Utc::now().timestamp_millis(), Ordering::Relaxed);
                match frame {
                    Message::Binary(b) => {
                        if matches!(ctx.flag("raw.book_frames"), Ok(true)) {
                            ctx.raw.write(ID, name, &hex::encode(&b));
                        }
                        let why = match sbe::decode(&b) {
                            Ok(Frame::Diff(d)) => match pairs.on_diff(d) {
                                Ok(true) => continue,
                                Ok(false) => return End::Done,
                                Err(why) => why,
                            },
                            Ok(Frame::Other(t)) => format!("сообщение SBE templateId {t} — не подписывались"),
                            Err(why) => why,
                        };
                        // Неразборчивый кадр: какой паре он принадлежал — неизвестно, пропуск мог задеть любую.
                        return End::Broken(format!("кадр SBE не разобран: {why}"));
                    }
                    Message::Text(t) => {
                        ctx.raw.write(ID, name, &t);
                        if let Some(end) = text(ctx, name, &t, &sent, &pairs).await {
                            return end;
                        }
                    }
                    Message::Ping(p) => {
                        if let Err(e) = sink.send(Message::Pong(p)).await {
                            return End::Broken(format!("отправка Pong: {e}"));
                        }
                    }
                    Message::Close(c) => return End::Broken(format!("сервер закрыл соединение: {c:?}")),
                    _ => {}
                }
            }
        }
    }
}

/// Событие serverShutdown (JSON).
#[derive(serde::Deserialize)]
struct Event {
    #[serde(default)]
    e: Option<String>,
}

/// Текстовый кадр: ответ на подписку, отказ (пары запроса — на страховку) или serverShutdown.
/// Возвращает: Some — сессию закончить. Ошибок нет.
async fn text(
    ctx: &Ctx,
    name: &str,
    t: &str,
    sent: &HashMap<String, Vec<String>>,
    pairs: &Pairs<'_>,
) -> Option<End> {
    match depth::parse(t) {
        Msg::Error { code, msg, id } => {
            for s in sent.get(&id).into_iter().flatten() {
                let key = s.split('@').next().unwrap_or_default();
                if let Some((a, _)) = pairs.by_key.get(key) {
                    pairs
                        .fast
                        .fast_lost(a, &format!("отказ подписки SBE, код {code}"), false);
                }
            }
            let m = format!("WebSocket {name}: отказ запроса id {id}, код {code}: {msg}");
            ctx.log(Level::Error, "venue.ws", m).await;
            None
        }
        Msg::Other => match serde_json::from_str::<Event>(t) {
            Ok(Event { e: Some(e) }) if e == "serverShutdown" => Some(End::Broken(
                "serverShutdown — сервер скоро закроет соединение".into(),
            )),
            _ => None,
        },
        Msg::Ack(_) | Msg::Depth { .. } | Msg::Bad(_) => None,
    }
}

//! Файл: venues/bybit/ws_conn.rs
//! Что делает: одна сессия публичного WebSocket спота Bybit: подключение, подписки пачками не больше 10 args
//! ({"op":"subscribe","req_id","args"}) с паузой, свой {"op":"ping"} раз в venue.ws_ping_ms, Pong на Ping-кадр,
//! локальные стаканы из снимка и изменений (local_book.rs) → память и ядру (try_send). Решение 428: та же сессия —
//! у соединения страховки (другая глубина, без сторожа ядра); кадры разбирает ws_book.rs / ws_lane.rs, выбор
//! источника стакана ядра — backup.rs. Разрыв номера u — стакан пары
//! недостоверен и пересподписка этой пары (новый снимок); второй разрыв той же пары раньше venue.ws_reconnect_pause_ms —
//! переподключение соединения с паузой (урок 12: без бесконечного круга ресинков). Отказ подписки — стаканы её пачки
//! недостоверны.
//! Решение 324: стакан ядру — gateway/core_out_book.rs (канал полон: venue.book_events_keep = да — последний кадр
//! пары ждёт места в общем наборе канала ядра; не задана — прежний сброс и счёт dropped).
//! Кто вызывает: ws.rs.
//! От чего зависит: ws_book.rs (состояние сессии и кадры), ws_lane.rs, depth.rs, book.rs, ctx.rs, gateway/feed_reg.rs,
//! tokio-tungstenite.
//! Настройки: из ws.rs (Cfg); raw.book_frames [bybit] — кадры стаканов в сырой журнал; venue.book_seq_strict — u строго +1.
//!
//! Официально (Connect https://bybit-exchange.github.io/docs/v5/ws/connect): «Spot can input up to 10 args for each
//! subscription request»; ответ подписки спота — {"success", "ret_msg", "req_id", "op":"subscribe"}. Тишина считается от
//! последнего кадра (как решение 98): pong — тоже кадр. Кадры стаканов в сыром журнале — только при raw.book_frames
//! (решение 109: поток стаканов нагружает диск); ответы op пишутся всегда.
//! Решение 202 (для всех бирж, 08.11): каждый кадр отмечается в учёте свежести (gateway/feed_reg.rs) — кадр стакана по
//! паре (снимок, изменение, повтор и кадр с разрывом тоже); номер u кадра передаётся только при глубине
//! book_print::U_LEVELS (решение 213: u REST связан с u потока только этой глубины), иначе подтверждение тихой пары — по
//! содержимому (book_print.rs). Команды сторожа ядра: «переподписать пару» — {"op":"unsubscribe","args":[тема]}, затем
//! {"op":"subscribe","args":[тема]} в этом же соединении (Connect, «Understanding WebSocket Filters: Unsubscription» —
//! «You can dynamically subscribe and unsubscribe from topics without unsubscribing from the WebSocket»; тот же вид, что у
//! пересподписки при разрыве u), соседи не трогаются, после подписки Bybit присылает snapshot («Once you have subscribed
//! successfully, you will receive a snapshot»). «Переподключить соединение» — сессия заканчивается, ws.rs подключается
//! заново с теми же парами.

use std::collections::VecDeque;
use std::sync::atomic::Ordering;

use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::Message;

use crate::core::types::Asset;
use crate::gateway::events::MarketEvent;
use crate::gateway::tap::now_ns;

use super::book::Books;
use super::ctx::Ctx;
use super::depth::channel;
use super::ws::{Cfg, End};
use super::ws_book::St;
use super::ws_give::Insure;
use crate::gateway::feed_health::Cmd;
use crate::gateway::feed_reg::ConnHandle;

/// Учёт свежести соединения и канал команд сторожа (решение 202). У соединения страховки (решение 428) — нет.
pub struct Watch<'a> {
    pub feed: &'a ConnHandle,
    pub cmd: &'a mut mpsc::UnboundedReceiver<Cmd>,
}

/// Сколько args в одном запросе подписки спота — из официальной документации («up to 10 args for each subscription»).
pub const SUB_ARGS_MAX: usize = 10;

/// Следующая команда сторожа; у соединения без сторожа — никогда. Ошибок нет.
async fn next_cmd(cmd: &mut Option<&mut mpsc::UnboundedReceiver<Cmd>>) -> Option<Cmd> {
    match cmd {
        Some(c) => c.recv().await,
        None => std::future::pending().await,
    }
}

/// Сессия: работает до обрыва. Принимает: окружение, имя соединения, пары, стаканы, канал ядра, настройки соединения,
/// страховку подписки, сторож (None — соединение страховки). Возвращает причину окончания.
#[allow(clippy::too_many_arguments)]
pub async fn session(
    ctx: &Ctx,
    name: &str,
    assets: &[(Asset, String)],
    books: &Books,
    out: &mpsc::Sender<MarketEvent>,
    cfg: Cfg,
    ins: &Insure,
    w: Option<Watch<'_>>,
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
    let (feed, mut cmd) = match w {
        Some(w) => (Some(w.feed), Some(w.cmd)),
        None => (None, None),
    };
    if let Some(feed) = feed {
        feed.session_started();
        for (a, _) in assets {
            ins.hub.main_session(&a.ticker, name);
        }
    }
    let mut st = St::new(ctx, name, assets, books, out, &cfg, ins, feed);
    // Пачки подписки по SUB_ARGS_MAX; req_id → символы пачки (чтобы снять их стаканы при отказе).
    let mut pending: VecDeque<(String, Vec<String>)> = st
        .topics
        .chunks(SUB_ARGS_MAX)
        .enumerate()
        .map(|(i, c)| (format!("s{}", i + 1), c.to_vec()))
        .collect();
    let mut sub_tick = tokio::time::interval(cfg.sub_gap);
    let mut ping = tokio::time::interval(cfg.ping);
    let mut last_rx = Instant::now();
    loop {
        let mut send: Vec<String> = vec![];
        tokio::select! {
            _ = sub_tick.tick(), if !pending.is_empty() => {
                let Some((id, args)) = pending.pop_front() else { continue };
                send.push(json!({ "op": "subscribe", "req_id": id, "args": args }).to_string());
                st.sent.insert(id, args);
            }
            Some(c) = next_cmd(&mut cmd) => match c {
                Cmd::Reconnect => return End::Broken("сторож пар: переподключение соединения (решение 202)".into()),
                Cmd::Resub(ticker) => {
                    let Some((_, sym)) = assets.iter().find(|(a, _)| a.ticker == ticker) else { continue };
                    let (topic, id) = (channel(cfg.levels, sym), format!("w-{sym}"));
                    // Отказ этой подписки — недостоверна только эта пара (ответ op с тем же req_id).
                    st.sent.insert(id.clone(), vec![topic.clone()]);
                    for op in ["unsubscribe", "subscribe"] {
                        send.push(json!({ "op": op, "req_id": id, "args": [topic.clone()] }).to_string());
                    }
                    if let Some(feed) = feed {
                        feed.resubscribed(&ticker);
                    }
                }
            },
            _ = ping.tick() => send.push(json!({ "op": "ping" }).to_string()),
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
                st.alive.store(chrono::Utc::now().timestamp_millis(), Ordering::Relaxed);
                if let Some(feed) = feed {
                    feed.alive();
                }
                match frame {
                    Message::Text(t) => {
                        let step = st.on_text(&t, rx_ns).await;
                        if let Some(end) = step.end {
                            return end;
                        }
                        send = step.send;
                    }
                    Message::Ping(p) => {
                        if let Err(e) = sink.send(Message::Pong(p)).await {
                            return End::Broken(format!("отправка Pong: {e}"));
                        }
                    }
                    Message::Close(c) => return End::Broken(format!("сервер закрыл соединение: {c:?}")),
                    _ => {}
                }
                if st.dropped > 0 && pending.is_empty() && st.dropped.is_power_of_two() {
                    tracing::warn!(conn = name, dropped = st.dropped, "ядро не успевает принимать стаканы Bybit");
                }
            }
        }
        for m in send {
            if let Err(e) = sink.send(Message::Text(m.into())).await {
                return End::Broken(format!("отправка в соединение: {e}"));
            }
        }
    }
}

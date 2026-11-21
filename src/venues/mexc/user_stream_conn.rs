//! Файл: venues/mexc/user_stream_conn.rs
//! Что делает: одна сессия потока аккаунта MEXC: подключение с listenKey, подписка spot@private.account.v3.api.pb,
//! продление ключа каждые venue.user_stream_keepalive_s, свой PING раз в venue.ws_ping_ms, ответ Pong на Ping-кадр,
//! запись сырых кадров (data/raw/mexc, соединение «account»; двоичные — hex; ключа в кадрах нет), разбор изменения
//! баланса (account_update.rs) → журнал и события ядру (MarketEvent::Balance). Ключ в тексты ошибок не попадает.
//! Кто вызывает: user_stream.rs.
//! От чего зависит: account_update.rs, user_key.rs, depth.rs (текстовые ответы), ctx.rs.
//! Настройки: приходят из user_stream.rs (Cfg).
//!
//! Официально: подписка {"method":"SUBSCRIPTION","params":["spot@private.account.v3.api.pb"]}, PING
//! {"method":"PING"} → PONG (https://www.mexc.com/api-docs/spot-v3/websocket-user-data-streams/spot-account-update,
//! …/websocket-market-streams/).

use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::Message;

use crate::gateway::events::MarketEvent;
use crate::storage::journal::Level;

use super::account_update::{self, Frame};
use super::ctx::{Ctx, ID};
use super::depth::{self, Msg};
use super::user_key;
use super::user_stream::Cfg;

/// Имя соединения в сыром журнале.
const NAME: &str = "account";
/// Канал изменений баланса спота (официальное имя).
const CHANNEL: &str = "spot@private.account.v3.api.pb";

/// Сессия до обрыва. Возвращает: Some(причина) — переподключиться; None — канал ядра закрыт.
pub async fn session(
    ctx: &Ctx,
    out: &mpsc::Sender<MarketEvent>,
    cfg: &Cfg,
    key: &str,
) -> Option<String> {
    let hide = |t: String| t.replace(key, "***");
    let url = format!("{}?listenKey={key}", cfg.url);
    let ws = match tokio::time::timeout(
        cfg.connect_timeout,
        tokio_tungstenite::connect_async(url.as_str()),
    )
    .await
    {
        Err(_) => return Some("не удалось подключиться за отведённое время".into()),
        Ok(Err(e)) => return Some(hide(format!("подключение: {e}"))),
        Ok(Ok((ws, _))) => ws,
    };
    let (mut sink, mut stream) = ws.split();
    let sub = json!({ "method": "SUBSCRIPTION", "params": [CHANNEL] });
    if let Err(e) = sink.send(Message::Text(sub.to_string().into())).await {
        return Some(hide(format!("отправка подписки: {e}")));
    }
    let mut extend = tokio::time::interval_at(Instant::now() + cfg.keepalive, cfg.keepalive);
    let mut ping = tokio::time::interval(cfg.ping);
    let mut last_rx = Instant::now();
    loop {
        tokio::select! {
            _ = extend.tick() => {
                if let Err(e) = user_key::extend(ctx, key).await {
                    return Some(hide(format!("listenKey не продлён: {}", e.message)));
                }
            }
            _ = ping.tick() => {
                let p = json!({ "method": "PING" });
                if let Err(e) = sink.send(Message::Text(p.to_string().into())).await {
                    return Some(format!("отправка PING: {e}"));
                }
            }
            _ = tokio::time::sleep_until(last_rx + cfg.silence) => {
                return Some(format!("нет кадров дольше {} мс", cfg.silence.as_millis()));
            }
            next = stream.next() => {
                let frame = match next {
                    None => return Some("соединение закрыто сервером".into()),
                    Some(Err(e)) => return Some(hide(format!("ошибка чтения: {e}"))),
                    Some(Ok(f)) => f,
                };
                last_rx = Instant::now();
                match frame {
                    Message::Binary(b) => {
                        ctx.raw.write(ID, NAME, &hex::encode(&b));
                        if !on_frame(ctx, out, &b).await {
                            return None;
                        }
                    }
                    Message::Text(t) => {
                        let t = hide(t.to_string());
                        ctx.raw.write(ID, NAME, &t);
                        if let Msg::Ack { code, msg } = depth::parse_text(&t)
                            && code != 0
                        {
                            return Some(format!("подписка {CHANNEL} отклонена, код {code}: {msg}"));
                        }
                    }
                    Message::Ping(p) => {
                        if let Err(e) = sink.send(Message::Pong(p)).await {
                            return Some(format!("отправка Pong: {e}"));
                        }
                    }
                    Message::Close(c) => return Some(format!("сервер закрыл соединение: {c:?}")),
                    _ => {}
                }
            }
        }
    }
}

/// Двоичный кадр → журнал и событие ядру. Возвращает: false — канал ядра закрыт. Ошибок нет.
async fn on_frame(ctx: &Ctx, out: &mpsc::Sender<MarketEvent>, data: &[u8]) -> bool {
    match account_update::parse(data, &ctx.venue()) {
        Frame::Other => true,
        Frame::Bad(why) => {
            log(
                ctx,
                Level::Error,
                format!("Изменение баланса не разобрано: {why}"),
                data,
            )
            .await;
            true
        }
        Frame::Update { reason, change } => {
            let what = change
                .as_ref()
                .map(|c| format!("{} {:+}", c.asset.ticker, c.delta))
                .unwrap_or_else(|| "без изменения общего баланса".into());
            log(
                ctx,
                Level::Info,
                format!("Получено изменение баланса type={reason}: {what}"),
                data,
            )
            .await;
            match change {
                Some(c) => out.send(MarketEvent::Balance(c)).await.is_ok(),
                None => true,
            }
        }
    }
}

/// Запись в журнал с сырым кадром (hex).
async fn log(ctx: &Ctx, level: Level, msg: String, raw: &[u8]) {
    let data = Some(json!({ "raw_hex": hex::encode(raw) }));
    let _ = ctx
        .journal
        .write(level, "venue.user_stream", Some(ID), msg, data)
        .await;
}

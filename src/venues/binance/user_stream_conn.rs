//! Файл: venues/binance/user_stream_conn.rs
//! Что делает: одна сессия потока аккаунта Binance через WebSocket API: подключение, запрос
//! userDataStream.subscribe.signature (подпись — sign.rs, параметры по алфавиту), Pong на Ping сервера, запись сырых
//! кадров (data/raw/binance, соединение «account»; ключ и подпись в журнал не попадают), разбор событий
//! (account_update.rs) → журнал и события ядру (MarketEvent::Balance); решение 251 — остатки outboundAccountPosition в
//! withdraw_ready.rs (ожидание доступности вывода).
//! Кто вызывает: user_stream.rs.
//! От чего зависит: account_update.rs, sign.rs, rest.rs (recv_window), ctx.rs.
//! Настройки: из user_stream.rs (Cfg); venue.recv_window_ms [binance].
//!
//! Официально (websocket-api «User Data Stream requests», «Request security»): запрос {"id", "method":
//! "userDataStream.subscribe.signature", "params": {apiKey, recvWindow, timestamp, signature}}, вес 2; ответ
//! {"id", "status": 200, "result": {"subscriptionId": 0}}; ошибка — status ≠ 200 и error {code, msg}; события —
//! {"subscriptionId", "event"}.

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::Message;

use crate::gateway::events::MarketEvent;
use crate::storage::journal::Level;

use super::account_update::{self, Event};
use super::ctx::{Ctx, ID};
use super::user_stream::Cfg;
use super::{rest, sign};

/// Имя соединения в сыром журнале.
const NAME: &str = "account";
/// id нашего запроса подписки.
const SUB_ID: &str = "sub";

/// Сессия до обрыва. Возвращает: Some(причина) — переподключиться; None — канал ядра закрыт.
pub async fn session(ctx: &Ctx, out: &mpsc::Sender<MarketEvent>, cfg: &Cfg) -> Option<String> {
    let request = match subscribe_request(ctx) {
        Ok(r) => r,
        Err(e) => return Some(e),
    };
    let ws = match tokio::time::timeout(
        cfg.connect_timeout,
        tokio_tungstenite::connect_async(cfg.url.as_str()),
    )
    .await
    {
        Err(_) => return Some("не удалось подключиться за отведённое время".into()),
        Ok(Err(e)) => return Some(format!("подключение: {e}")),
        Ok(Ok((ws, _))) => ws,
    };
    let (mut sink, mut stream) = ws.split();
    if let Err(e) = sink.send(Message::Text(request.into())).await {
        return Some(format!("отправка подписки: {e}"));
    }
    let mut last_rx = Instant::now();
    loop {
        tokio::select! {
            _ = tokio::time::sleep_until(last_rx + cfg.silence) => {
                return Some(format!("нет кадров дольше {} мс", cfg.silence.as_millis()));
            }
            next = stream.next() => {
                let frame = match next {
                    None => return Some("соединение закрыто сервером".into()),
                    Some(Err(e)) => return Some(format!("ошибка чтения: {e}")),
                    Some(Ok(f)) => f,
                };
                last_rx = Instant::now();
                match frame {
                    Message::Text(t) => {
                        ctx.raw.write(ID, NAME, &t);
                        if let Some(why) = reply_error(&t) {
                            return Some(why);
                        }
                        match on_event(ctx, out, &t).await {
                            Ok(true) => {}
                            Ok(false) => return None,
                            Err(why) => return Some(why),
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

/// Текст запроса подписки с подписью. Ошибка — текст (ключи, recvWindow, секрет).
fn subscribe_request(ctx: &Ctx) -> Result<String, String> {
    let (key, secret) = ctx.keys().map_err(|e| e.message)?;
    let recv = rest::recv_window(ctx).map_err(|e| e.message)?;
    let params = vec![
        ("apiKey", key.to_string()),
        ("recvWindow", recv.to_string()),
        (
            "timestamp",
            chrono::Utc::now().timestamp_millis().to_string(),
        ),
    ];
    let signature = sign::hmac_hex(&secret, &sign::ws_payload(&params))?;
    let mut p = serde_json::Map::new();
    for (k, v) in params {
        let v = if k == "apiKey" {
            Value::String(v)
        } else {
            Value::Number(v.parse().map_err(|_| format!("{k} не число"))?)
        };
        p.insert(k.to_string(), v);
    }
    p.insert("signature".into(), Value::String(signature));
    let req = json!({"id": SUB_ID, "method": "userDataStream.subscribe.signature", "params": p});
    Ok(req.to_string())
}

/// Ответ на наш запрос с ошибкой (status ≠ 200) → причина. Ошибок нет.
fn reply_error(t: &str) -> Option<String> {
    let v: Value = serde_json::from_str(t).ok()?;
    if v.get("id").and_then(Value::as_str) != Some(SUB_ID) {
        return None;
    }
    let status = v.get("status").and_then(Value::as_i64)?;
    (status != 200).then(|| {
        let err = v.get("error").map(Value::to_string).unwrap_or_default();
        format!("подписка userDataStream.subscribe.signature отклонена, status {status}: {err}")
    })
}

/// Событие → журнал и ядру. Возвращает: Ok(false) — канал ядра закрыт; Err — подписка закрыта сервером.
async fn on_event(ctx: &Ctx, out: &mpsc::Sender<MarketEvent>, t: &str) -> Result<bool, String> {
    match account_update::parse(t, &ctx.venue()) {
        Event::Other(_) => Ok(true),
        // Решение 251: свободный остаток — ожиданию доступности вывода (withdraw_ready.rs), ядру не отдаётся.
        Event::Position(list, at) => {
            super::withdraw_ready::on_position(&list, at);
            Ok(true)
        }
        Event::Terminated => Err("сервер закрыл подписку (eventStreamTerminated)".into()),
        Event::Bad(why) => {
            log(
                ctx,
                Level::Error,
                format!("Событие аккаунта не разобрано: {why}"),
                t,
            )
            .await;
            Ok(true)
        }
        Event::Balance(c) => {
            let m = format!(
                "Получено изменение баланса: {} {:+}",
                c.asset.ticker, c.delta
            );
            log(ctx, Level::Info, m, t).await;
            Ok(out.send(MarketEvent::Balance(c)).await.is_ok())
        }
    }
}

/// Запись в журнал с сырым кадром. Ошибок нет.
async fn log(ctx: &Ctx, level: Level, msg: String, raw: &str) {
    let data = Some(json!({ "raw": raw }));
    let _ = ctx
        .journal
        .write(level, "venue.user_stream", Some(ID), msg, data)
        .await;
}

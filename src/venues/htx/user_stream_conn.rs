//! Файл: venues/htx/user_stream_conn.rs
//! Что делает: одна сессия потока аккаунта HTX (WebSocket v2): подключение, вход подписью (sign.rs, ws_auth), после
//! ответа code 200 — подписка accounts.update#2, ответ pong на каждый ping сервера, тишина дольше venue.ws_silence_ms —
//! обрыв; сырые кадры — в журнал (data/raw/htx, соединение «account»; подпись входа в журнал не пишется), изменение
//! баланса (account_update.rs) → журнал и событие ядру MarketEvent::Balance.
//! Кто вызывает: user_stream.rs.
//! От чего зависит: sign.rs, account_update.rs, ctx.rs, tokio-tungstenite.
//! Настройки: приходят из user_stream.rs (Cfg).
//!
//! Официально («Websocket Account and Order», https://huobiapi.github.io/docs/spot/v1/en/): кадры не сжаты («not
//! compressed by GZIP»); вход {"action":"req","ch":"auth","params":{…}} → {"action":"req","code":200,"ch":"auth"};
//! подписка {"action":"sub","ch":"accounts.update#2"} → code 200; ping {"action":"ping","data":{"ts":n}} → ответ
//! {"action":"pong","data":{"ts":n}}; данные {"action":"push","ch":…,"data":{…}}; ошибка — code не 200 (например 2002,
//! 4000 «too.many.request»).

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::Message;

use crate::gateway::events::MarketEvent;
use crate::storage::journal::Level;

use super::account_update::{self, Last};
use super::ctx::{Ctx, ID};
use super::sign;

/// Имя соединения в сыром журнале.
const NAME: &str = "account";
/// Тема изменений баланса: режим 2 — balance и available вместе.
pub const TOPIC: &str = "accounts.update#2";

/// Настройки сессии.
pub struct Cfg {
    pub url: String,
    pub timeout: Duration,
    pub silence: Duration,
}

/// Сессия до обрыва. Принимает: окружение, канал ядра, id спот-счёта, настройки.
/// Возвращает: Some(причина) — переподключиться; None — канал ядра закрыт.
pub async fn session(
    ctx: &Ctx,
    out: &mpsc::Sender<MarketEvent>,
    spot_id: i64,
    cfg: Cfg,
) -> Option<String> {
    let (host, path) = match sign::host_path(&cfg.url) {
        Ok(hp) => hp,
        Err(e) => return Some(format!("venue.ws_private_url: {e}")),
    };
    let auth = match ctx
        .keys()
        .and_then(|k| sign::ws_auth(&k, &host, &path, chrono::Utc::now()))
    {
        Ok(a) => a.to_string(),
        Err(e) => return Some(e.message),
    };
    let ws = match tokio::time::timeout(
        cfg.timeout,
        tokio_tungstenite::connect_async(cfg.url.as_str()),
    )
    .await
    {
        Err(_) => return Some("не удалось подключиться за отведённое время".into()),
        Ok(Err(e)) => return Some(format!("подключение: {e}")),
        Ok(Ok((ws, _))) => ws,
    };
    let (mut sink, mut stream) = ws.split();
    if let Err(e) = sink.send(Message::Text(auth.into())).await {
        return Some(format!("отправка входа: {e}"));
    }
    let mut last: Last = Last::new();
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
                let text = match frame {
                    Message::Text(s) => s.to_string(),
                    Message::Ping(p) => {
                        if let Err(e) = sink.send(Message::Pong(p)).await {
                            return Some(format!("отправка Pong: {e}"));
                        }
                        continue;
                    }
                    Message::Close(c) => return Some(format!("сервер закрыл соединение: {c:?}")),
                    _ => continue,
                };
                ctx.raw.write(ID, NAME, &text);
                let Ok(v) = serde_json::from_str::<Value>(&text) else { continue };
                let action = v.get("action").and_then(Value::as_str).unwrap_or_default();
                let ch = v.get("ch").and_then(Value::as_str).unwrap_or_default();
                let ok = v.get("code").and_then(Value::as_i64).is_none_or(|c| c == 200);
                match action {
                    "ping" => {
                        let pong = json!({"action": "pong", "data": v.get("data").cloned().unwrap_or(Value::Null)});
                        if let Err(e) = sink.send(Message::Text(pong.to_string().into())).await {
                            return Some(format!("отправка pong: {e}"));
                        }
                    }
                    _ if !ok => return Some(format!("ошибка HTX ({action} {ch}): {text}")),
                    "req" if ch == "auth" => {
                        let sub = json!({"action": "sub", "ch": TOPIC}).to_string();
                        if let Err(e) = sink.send(Message::Text(sub.into())).await {
                            return Some(format!("отправка подписки: {e}"));
                        }
                    }
                    "push" if ch == TOPIC => {
                        let data = v.get("data").cloned().unwrap_or(Value::Null);
                        if !on_push(ctx, out, &data, spot_id, &mut last).await {
                            return None;
                        }
                    }
                    _ => {}
                }
            }
        }
    }
}

/// Кадр изменения баланса → журнал и событие ядру. Возвращает: false — канал ядра закрыт. Ошибок нет.
async fn on_push(
    ctx: &Ctx,
    out: &mpsc::Sender<MarketEvent>,
    data: &Value,
    spot_id: i64,
    last: &mut Last,
) -> bool {
    match account_update::parse(data, spot_id, last, &ctx.venue()) {
        Err(why) => {
            let m = format!("Изменение баланса HTX не разобрано: {why}");
            ctx.log(Level::Error, "venue.user_stream", m).await;
            true
        }
        Ok((_, None)) => true,
        Ok((what, Some(c))) => {
            let m = format!(
                "Получено изменение баланса HTX changeType={what}: {} {:+}",
                c.asset.ticker, c.delta
            );
            ctx.log(Level::Info, "venue.user_stream", m).await;
            out.send(MarketEvent::Balance(c)).await.is_ok()
        }
    }
}

//! Файл: venues/bybit/user_stream_conn.rs
//! Что делает: одна сессия приватного потока Bybit: подключение к venue.ws_private_url, {"op":"auth"} с подписью,
//! подписка order.spot, execution.spot, wallet; свой {"op":"ping"} раз в venue.ws_ping_ms, Pong на Ping-кадр; записи
//! ордеров и исполнений — в память (private_cache.rs); все кадры — в сырой журнал (соединение «private»; ключа и подписи
//! в кадрах сервера нет). Кадры wallet только пишутся (см. user_stream.rs — почему не события ядру).
//! Кто вызывает: user_stream.rs.
//! От чего зависит: private_cache.rs, depth.rs (ответы op), ctx.rs, hmac, sha2.
//! Настройки: приходят из user_stream.rs (Cfg); секреты bybit.api_key, bybit.api_secret.
//!
//! Официально (Connect https://bybit-exchange.github.io/docs/v5/ws/connect, раздел Authentication): {"op":"auth",
//! "args":[api_key, expires, signature]}, «expires; is greater than your current timestamp», подпись — HMAC SHA256 от
//! «GET/realtime{expires}» (hex); ответ {"success":true,"op":"auth"}. Темы — Order («Categorised Topic: order.spot»),
//! Execution («execution.spot»), Wallet («wallet»): {"topic", "creationTime", "data":[…]}.

use futures_util::{SinkExt, StreamExt};
use hmac::{Hmac, KeyInit, Mac};
use serde_json::{Value, json};
use sha2::Sha256;
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::Message;

use super::ctx::{Ctx, ID};
use super::user_stream::Cfg;

/// Имя соединения в сыром журнале.
const NAME: &str = "private";
/// Темы приватного потока (официальные имена).
const TOPICS: [&str; 3] = ["order.spot", "execution.spot", "wallet"];

/// Сессия до обрыва. Возвращает: причину (переподключиться).
pub async fn session(ctx: &Ctx, cfg: &Cfg) -> String {
    let (key, secret) = match ctx.keys() {
        Ok(k) => k,
        Err(e) => return e.message,
    };
    let ws = match tokio::time::timeout(
        cfg.connect_timeout,
        tokio_tungstenite::connect_async(cfg.url.as_str()),
    )
    .await
    {
        Err(_) => return "не удалось подключиться за отведённое время".into(),
        Ok(Err(e)) => return format!("подключение: {e}"),
        Ok(Ok((ws, _))) => ws,
    };
    let (mut sink, mut stream) = ws.split();
    let expires = chrono::Utc::now().timestamp_millis() + cfg.sign_ms;
    let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(secret.as_bytes()) else {
        return "пустой секретный ключ Bybit".into();
    };
    mac.update(format!("GET/realtime{expires}").as_bytes());
    let sign = hex::encode(mac.finalize().into_bytes());
    let auth = json!({ "op": "auth", "args": [key.as_str(), expires, sign] });
    if let Err(e) = sink.send(Message::Text(auth.to_string().into())).await {
        return format!("отправка auth: {e}");
    }
    let mut ping = tokio::time::interval(cfg.ping);
    let mut last_rx = Instant::now();
    loop {
        tokio::select! {
            _ = ping.tick() => {
                if let Err(e) = sink.send(Message::Text(json!({ "op": "ping" }).to_string().into())).await {
                    return format!("отправка ping: {e}");
                }
            }
            _ = tokio::time::sleep_until(last_rx + cfg.silence) => {
                return format!("нет кадров дольше {} мс", cfg.silence.as_millis());
            }
            next = stream.next() => {
                let frame = match next {
                    None => return "соединение закрыто сервером".into(),
                    Some(Err(e)) => return format!("ошибка чтения: {e}"),
                    Some(Ok(f)) => f,
                };
                last_rx = Instant::now();
                let t = match frame {
                    Message::Text(t) => t,
                    Message::Ping(p) => {
                        if let Err(e) = sink.send(Message::Pong(p)).await {
                            return format!("отправка Pong: {e}");
                        }
                        continue;
                    }
                    Message::Close(c) => return format!("сервер закрыл соединение: {c:?}"),
                    _ => continue,
                };
                ctx.raw.write(ID, NAME, &t);
                if let Some(a) = super::depth::ack(&t) {
                    let op = a.op.unwrap_or_default();
                    let ok = a.success.unwrap_or(op == "pong");
                    if !ok {
                        return format!("{op} отклонён: {}", a.ret_msg.unwrap_or_default());
                    }
                    if op == "auth" {
                        let sub = json!({ "op": "subscribe", "args": TOPICS });
                        if let Err(e) = sink.send(Message::Text(sub.to_string().into())).await {
                            return format!("отправка подписки: {e}");
                        }
                    }
                    continue;
                }
                on_data(ctx, &t);
            }
        }
    }
}

/// Кадр данных → память: order.spot — записи ордеров, execution.spot — исполнения. Ошибок нет (кадр уже в сыром журнале).
fn on_data(ctx: &Ctx, text: &str) {
    let Ok(v) = serde_json::from_str::<Value>(text) else {
        return;
    };
    let topic = v.get("topic").and_then(Value::as_str).unwrap_or_default();
    let Some(list) = v.get("data").and_then(Value::as_array) else {
        return;
    };
    for item in list {
        match topic {
            "order.spot" | "order" => ctx.private.put_order(item),
            "execution.spot" | "execution" => ctx.private.put_fill(item),
            _ => {}
        }
    }
}

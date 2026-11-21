//! Файл: venues/kucoin/user_stream_conn.rs
//! Что делает: одна сессия потока аккаунта KuCoin: подключение по приватному токену, welcome, подписка на /account/balance
//! и /spotMarket/tradeOrdersV2 (privateChannel), свой ping раз в pingInterval, тишина дольше pingInterval + pingTimeout —
//! обрыв; сырые кадры — в журнал (data/raw/kucoin, соединение «account»; токена в кадрах нет), изменение баланса
//! (account_update.rs) → журнал и событие ядру MarketEvent::Balance.
//! Кто вызывает: user_stream.rs.
//! От чего зависит: ws_conn.rs (open), depth.rs (разбор кадра), account_update.rs, bullet.rs, ctx.rs.
//! Настройки: нет своих.
//!
//! Официально (спецификация SDK spec/ws/openapi-spot-private.json, x-topic): «/account/balance» — «You will receive this
//! message when an account balance changes»; «/spotMarket/tradeOrdersV2» — «This topic will push all change events of
//! your orders». Итог ордера бот берёт REST-запросом ордера (order_state.rs); события ордеров пока только пишутся
//! в сырой журнал (разбор — после живой записи, docs/TASKS.md).

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::Message;

use crate::gateway::events::MarketEvent;
use crate::gateway::tap::now_ns;
use crate::storage::journal::Level;

use super::bullet::Token;
use super::ctx::{Ctx, ID};
use super::depth::{self, Msg};

/// Имя соединения в сыром журнале.
const NAME: &str = "account";
/// Приватные темы (официальные имена).
const TOPICS: [&str; 2] = ["/account/balance", "/spotMarket/tradeOrdersV2"];

/// Сессия до обрыва. Возвращает: Some(причина) — переподключиться; None — канал ядра закрыт.
pub async fn session(
    ctx: &Ctx,
    out: &mpsc::Sender<MarketEvent>,
    t: &Token,
    timeout: Duration,
) -> Option<String> {
    let ws = match super::ws_conn::open(t, timeout).await {
        Ok(ws) => ws,
        Err(e) => return Some(e),
    };
    let (mut sink, mut stream) = ws.split();
    for (i, topic) in TOPICS.iter().enumerate() {
        let sub = json!({"id": format!("acc{i}"), "type": "subscribe", "topic": topic, "privateChannel": true, "response": true});
        if let Err(e) = sink.send(Message::Text(sub.to_string().into())).await {
            return Some(format!("отправка подписки {topic}: {e}"));
        }
    }
    let mut ping = tokio::time::interval_at(Instant::now() + t.ping, t.ping);
    let silence = t.ping + t.ping_timeout;
    let mut last_rx = Instant::now();
    loop {
        tokio::select! {
            _ = ping.tick() => {
                let p = json!({"id": now_ns().to_string(), "type": "ping"});
                if let Err(e) = sink.send(Message::Text(p.to_string().into())).await {
                    return Some(format!("отправка ping: {e}"));
                }
            }
            _ = tokio::time::sleep_until(last_rx + silence) => {
                return Some(format!("нет кадров дольше {} мс", silence.as_millis()));
            }
            next = stream.next() => {
                let frame = match next {
                    None => return Some("соединение закрыто сервером".into()),
                    Some(Err(e)) => return Some(format!("ошибка чтения: {e}")),
                    Some(Ok(f)) => f,
                };
                last_rx = Instant::now();
                match frame {
                    Message::Text(s) => {
                        ctx.raw.write(ID, NAME, &s);
                        if !on_text(ctx, out, &s).await {
                            return None;
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

/// Текстовый кадр → журнал и событие ядру. Возвращает: false — канал ядра закрыт. Ошибок нет.
async fn on_text(ctx: &Ctx, out: &mpsc::Sender<MarketEvent>, s: &str) -> bool {
    match depth::parse(s) {
        Msg::Error { text, .. } => {
            ctx.log(
                Level::Error,
                "venue.user_stream",
                format!("Поток аккаунта: ошибка KuCoin: {text}"),
            )
            .await;
            true
        }
        Msg::Data { topic, data, .. } if topic == TOPICS[0] => {
            match super::account_update::parse(&data, &ctx.venue()) {
                Err(why) => {
                    ctx.log(
                        Level::Error,
                        "venue.user_stream",
                        format!("Изменение баланса не разобрано: {why}"),
                    )
                    .await;
                    true
                }
                Ok((event, change)) => {
                    let what = change
                        .as_ref()
                        .map(|c| format!("{} {:+}", c.asset.ticker, c.delta))
                        .unwrap_or_else(|| "без изменения общего баланса main/trade".into());
                    let m = format!("Получено изменение баланса relationEvent={event}: {what}");
                    ctx.log(Level::Info, "venue.user_stream", m).await;
                    match change {
                        Some(c) => out.send(MarketEvent::Balance(c)).await.is_ok(),
                        None => true,
                    }
                }
            }
        }
        _ => true,
    }
}

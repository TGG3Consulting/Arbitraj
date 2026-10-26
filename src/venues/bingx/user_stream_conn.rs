//! Файл: venues/bingx/user_stream_conn.rs
//! Что делает: одна сессия потока аккаунта BingX (решение 76): подключение с listenKey, подписка ACCOUNT_UPDATE,
//! продление ключа каждые venue.user_stream_keepalive_s, ответ «Pong» на ping, запись сырых кадров (data/raw/bingx,
//! соединение «account»; ключа в кадрах нет), разбор ACCOUNT_UPDATE (account_update.rs) → журнал «получено
//! ACCOUNT_UPDATE …» и события баланса ядру (MarketEvent::Balance). Ключ в тексты ошибок не попадает.
//! Кто вызывает: user_stream.rs.
//! От чего зависит: account_update.rs, user_key.rs, depth.rs (gunzip, ping, ответ на подписку), ctx.rs.
//! Настройки: приходят из user_stream.rs (Cfg).
//!
//! Официально: сообщения сжаты GZIP, на «Ping» отвечать «Pong» (docs-v3 WebSocket, FAQ-05); ответ на подписку
//! {id, code, msg}; истечение ключа — событие listenKeyExpired (api-ai-skills/skills/references/websocket.md).

use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use tokio::sync::mpsc;
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
    let sub = json!({ "id": "account-1", "reqType": "sub", "dataType": "ACCOUNT_UPDATE" });
    if let Err(e) = sink.send(Message::Text(sub.to_string().into())).await {
        return Some(hide(format!("отправка подписки: {e}")));
    }
    let mut extend =
        tokio::time::interval_at(tokio::time::Instant::now() + cfg.keepalive, cfg.keepalive);
    loop {
        tokio::select! {
            _ = extend.tick() => {
                if let Err(e) = user_key::extend(ctx, key).await {
                    return Some(hide(format!("listenKey не продлён: {}", e.message)));
                }
            }
            next = tokio::time::timeout(cfg.silence, stream.next()) => {
                let frame = match next {
                    Err(_) => return Some(format!("нет сообщений дольше {} мс", cfg.silence.as_millis())),
                    Ok(None) => return Some("соединение закрыто сервером".into()),
                    Ok(Some(Err(e))) => return Some(hide(format!("ошибка чтения: {e}"))),
                    Ok(Some(Ok(f))) => f,
                };
                let text = match frame {
                    Message::Binary(b) => match depth::gunzip(&b) {
                        Ok(t) => t,
                        Err(e) => return Some(e),
                    },
                    Message::Text(t) => t.to_string(),
                    Message::Close(c) => return Some(format!("сервер закрыл соединение: {c:?}")),
                    _ => continue,
                };
                let text = hide(text);
                ctx.raw.write(ID, NAME, &text);
                match depth::parse(&text) {
                    Msg::Ping => {
                        if let Err(e) = sink.send(Message::Text("Pong".into())).await {
                            return Some(format!("отправка Pong: {e}"));
                        }
                        continue;
                    }
                    Msg::SubAck { code, msg, .. } if code != 0 => {
                        return Some(format!("подписка ACCOUNT_UPDATE отклонена, код {code}: {msg}"));
                    }
                    _ => {}
                }
                if text.contains("\"listenKeyExpired\"") {
                    return Some("listenKey истёк (listenKeyExpired)".into());
                }
                if !on_frame(ctx, out, &text).await {
                    return None;
                }
            }
        }
    }
}

/// Кадр ACCOUNT_UPDATE → журнал и события ядру. Возвращает: false — канал ядра закрыт. Ошибок нет.
async fn on_frame(ctx: &Ctx, out: &mpsc::Sender<MarketEvent>, text: &str) -> bool {
    match account_update::parse(text, &ctx.venue()) {
        Frame::Other => true,
        Frame::Bad(why) => {
            log(
                ctx,
                Level::Error,
                format!("ACCOUNT_UPDATE не разобран: {why}"),
                text,
            )
            .await;
            true
        }
        Frame::Update {
            wrapped,
            reason,
            changes,
        } => {
            let form = if wrapped {
                "в обёртке data"
            } else {
                "без обёртки"
            };
            let list: Vec<String> = changes
                .iter()
                .map(|c| format!("{} {:+}", c.asset.ticker, c.delta))
                .collect();
            let msg = format!(
                "Получено ACCOUNT_UPDATE m={reason} ({form}): {}",
                list.join(", ")
            );
            log(ctx, Level::Info, msg, text).await;
            for c in changes {
                if out.send(MarketEvent::Balance(c)).await.is_err() {
                    return false;
                }
            }
            true
        }
    }
}

/// Запись в журнал с сырым кадром.
async fn log(ctx: &Ctx, level: Level, msg: String, raw: &str) {
    let data = Some(json!({ "raw": raw }));
    let _ = ctx
        .journal
        .write(level, "venue.user_stream", Some(ID), msg, data)
        .await;
}

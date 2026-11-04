//! Файл: venues/okx/user_stream_conn.rs
//! Что делает: одна сессия потока аккаунта OKX на адресе business: подключение, вход (login, sign.rs), подписка на
//! deposit-info, свой «ping» после venue.ws_ping_ms без кадров, тишина дольше venue.ws_silence_ms — обрыв; сырые кадры —
//! в журнал (data/raw/okx, соединение «account»; запрос входа с ключом не пишется), зачисление депозита
//! (deposit_event.rs) → журнал и событие ядру MarketEvent::Balance.
//! Кто вызывает: user_stream.rs.
//! От чего зависит: ws_req.rs (open), ws.rs (config_ping), depth.rs (разбор кадра), deposit_event.rs, sign.rs, ctx.rs.
//! Настройки: venue.ws_business_url [okx]; сроки — ws.rs.
//!
//! Официально (https://www.okx.com/docs-v5/en/, «Overview → WebSocket → Login»): {"op":"login","args":[{apiKey,
//! passphrase, timestamp, sign}]}, «The request will expire 30 seconds after the timestamp»; успех — {"event":"login",
//! "code":"0"}, отказ — {"event":"error","code":"60009","msg":"Login failed."} (60004 «Invalid timestamp», 60005 «Invalid
//! apiKey»). Подписка deposit-info — {"op":"subscribe","args":[{"channel":"deposit-info"}]} («ccy» — необязателен).

use std::collections::HashSet;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::Message;

use crate::gateway::events::MarketEvent;
use crate::storage::journal::Level;

use super::ctx::{Ctx, ID};
use super::depth::{self, Msg};

/// Имя соединения в сыром журнале.
const NAME: &str = "account";
/// Канал зачислений депозитов (официальное имя).
const CHANNEL: &str = "deposit-info";

/// Настройки сессии.
pub struct Cfg {
    pub url: String,
    pub ping: Duration,
    pub silence: Duration,
    pub timeout: Duration,
}

/// Читает настройки. Ошибка — текст «не задана …».
pub fn config(ctx: &Ctx) -> Result<Cfg, String> {
    let (ping, silence) = super::ws::config_ping(ctx)?;
    Ok(Cfg {
        url: ctx.text("venue.ws_business_url").map_err(|e| e.message)?,
        ping,
        silence,
        timeout: ctx.http_timeout().map_err(|e| e.message)?,
    })
}

/// Сессия до обрыва. Принимает: окружение, канал ядра, настройки, отданные depId. Возвращает: Some(причина) —
/// переподключиться; None — канал ядра закрыт.
pub async fn session(
    ctx: &Ctx,
    out: &mpsc::Sender<MarketEvent>,
    cfg: &Cfg,
    seen: &mut HashSet<String>,
) -> Option<String> {
    let keys = match ctx.keys() {
        Ok(k) => k,
        Err(e) => return Some(e.message),
    };
    let ws = match super::ws_req::open(&cfg.url, cfg.timeout).await {
        Ok(ws) => ws,
        Err(e) => return Some(e),
    };
    // Время входа — сразу перед отправкой: «The request will expire 30 seconds after the timestamp».
    let login = match super::sign::ws_login(&keys, chrono::Utc::now().timestamp()) {
        Ok(l) => l,
        Err(e) => return Some(e.message),
    };
    drop(keys);
    let (mut sink, mut stream) = ws.split();
    if let Err(e) = sink.send(Message::Text(login.to_string().into())).await {
        return Some(format!("отправка входа: {e}"));
    }
    // Ответ на вход: до него подписка не принимается.
    let answer = tokio::time::timeout(cfg.timeout, async {
        while let Some(Ok(f)) = stream.next().await {
            if let Message::Text(s) = f
                && let Msg::Event {
                    event, code, text, ..
                } = depth::parse(&s)
            {
                return if event == "login" && code == "0" {
                    Ok(())
                } else {
                    Err(text)
                };
            }
        }
        Err("соединение закрыто до ответа на вход".into())
    });
    match answer.await {
        Ok(Ok(())) => {}
        Ok(Err(t)) => return Some(format!("вход не принят: {t}")),
        Err(_) => return Some("нет ответа на вход за http.timeout_ms".into()),
    }
    let sub = json!({"op": "subscribe", "args": [{"channel": CHANNEL}]});
    if let Err(e) = sink.send(Message::Text(sub.to_string().into())).await {
        return Some(format!("отправка подписки {CHANNEL}: {e}"));
    }
    let (mut last_rx, mut pinged) = (Instant::now(), false);
    loop {
        tokio::select! {
            _ = tokio::time::sleep_until(last_rx + cfg.ping), if !pinged => {
                pinged = true;
                if let Err(e) = sink.send(Message::Text("ping".into())).await {
                    return Some(format!("отправка ping: {e}"));
                }
            }
            _ = tokio::time::sleep_until(last_rx + cfg.silence) => {
                return Some(format!("нет кадров дольше {} мс", cfg.silence.as_millis()));
            }
            next = stream.next() => {
                let frame = match next {
                    None => return Some("соединение закрыто сервером".into()),
                    Some(Err(e)) => return Some(format!("ошибка чтения: {e}")),
                    Some(Ok(f)) => f,
                };
                (last_rx, pinged) = (Instant::now(), false);
                match frame {
                    Message::Text(s) => {
                        ctx.raw.write(ID, NAME, &s);
                        if !on_text(ctx, out, &s, seen).await {
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
async fn on_text(
    ctx: &Ctx,
    out: &mpsc::Sender<MarketEvent>,
    s: &str,
    seen: &mut HashSet<String>,
) -> bool {
    match depth::parse(s) {
        Msg::Event { event, text, .. } if event == "error" => {
            ctx.log(
                Level::Error,
                "venue.user_stream",
                format!("Поток аккаунта: ошибка OKX: {text}"),
            )
            .await;
            true
        }
        Msg::Data { channel, data } if channel == CHANNEL => {
            for row in data.as_array().map(Vec::as_slice).unwrap_or_default() {
                match super::deposit_event::parse(row, &ctx.venue(), seen) {
                    Err(why) => {
                        let m = format!("Депозит из deposit-info не разобран: {why}");
                        ctx.log(Level::Error, "venue.user_stream", m).await;
                    }
                    Ok((state, dep, change)) => {
                        let what = change
                            .as_ref()
                            .map(|c| format!("зачислено {} {:+}", c.asset.ticker, c.delta))
                            .unwrap_or_else(|| "без зачисления для ядра".into());
                        let m = format!("Депозит depId={dep}, state={state}: {what}");
                        ctx.log(Level::Info, "venue.user_stream", m).await;
                        if let Some(c) = change
                            && out.send(MarketEvent::Balance(c)).await.is_err()
                        {
                            return false;
                        }
                    }
                }
            }
            true
        }
        _ => true,
    }
}

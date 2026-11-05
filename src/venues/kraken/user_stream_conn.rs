//! Файл: venues/kraken/user_stream_conn.rs
//! Что делает: одна сессия приватного WebSocket v2 Kraken: новый токен (POST /0/private/GetWebSocketsToken) на каждое
//! подключение, подписка balances, свой ping после venue.ws_ping_ms без кадров, тишина дольше venue.ws_silence_ms —
//! обрыв; сырые кадры — в журнал (data/raw/kraken, соединение «account»; запрос подписки с токеном не пишется);
//! депозит / вывод спота (balance_event.rs) → журнал и событие ядру MarketEvent::Balance.
//! Кто вызывает: user_stream.rs.
//! От чего зависит: ws_req.rs (open, balances, ping), ws.rs (config_ping), depth.rs (разбор), balance_event.rs, rest.rs,
//! ctx.rs.
//! Настройки: venue.ws_private_url [kraken]; сроки — ws.rs.
//!
//! Официально: https://docs.kraken.com/exchange/guides/websockets/authentication.md — токен «must be used within 15
//! minutes of creation», на каждое подключение — новый; право ключа «WebSocket interface — On» (create-ws-token);
//! https://docs.kraken.com/exchange/api-reference/spot-websocket-v2/balances.md — подписка {"method":"subscribe",
//! "params":{"channel":"balances","token":…}}, ответ success true/false.

use std::collections::HashSet;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::Message;

use crate::gateway::events::MarketEvent;
use crate::storage::journal::Level;

use super::ctx::{Ctx, ID};
use super::depth::{self, Msg};
use super::errors::Op;
use super::{num, rest, ws_req};

/// Имя соединения в сыром журнале.
const NAME: &str = "account";

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
        url: ctx.text("venue.ws_private_url").map_err(|e| e.message)?,
        ping,
        silence,
        timeout: ctx.http_timeout().map_err(|e| e.message)?,
    })
}

/// Сессия до обрыва. Принимает: окружение, канал ядра, настройки, отданные ledger_id. Возвращает: Some(причина) —
/// переподключиться; None — канал ядра закрыт.
pub async fn session(
    ctx: &Ctx,
    out: &mpsc::Sender<MarketEvent>,
    cfg: &Cfg,
    seen: &mut HashSet<String>,
) -> Option<String> {
    let token = match rest::private(ctx, "GetWebSocketsToken", vec![], Op::Other).await {
        Ok(r) => match num::text_opt(&r, "token") {
            Some(t) => t,
            None => return Some("GetWebSocketsToken: в ответе нет token".into()),
        },
        Err(e) => return Some(format!("токен WebSocket не получен: {}", e.message)),
    };
    let ws = match ws_req::open(&cfg.url, cfg.timeout).await {
        Ok(ws) => ws,
        Err(e) => return Some(e),
    };
    let (mut sink, mut stream) = ws.split();
    if let Err(e) = sink
        .send(Message::Text(ws_req::balances(&token).into()))
        .await
    {
        return Some(format!("отправка подписки balances: {e}"));
    }
    drop(token);
    let (mut last_rx, mut pinged, mut req_id) = (Instant::now(), false, 0u64);
    loop {
        tokio::select! {
            _ = tokio::time::sleep_until(last_rx + cfg.ping), if !pinged => {
                pinged = true;
                req_id += 1;
                if let Err(e) = sink.send(Message::Text(ws_req::ping(req_id).into())).await {
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
                        match on_text(ctx, out, &s, seen).await {
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

/// Текстовый кадр → журнал и событие ядру. Возвращает: Ok(false) — канал ядра закрыт; Err — подписка отклонена
/// (переподключиться с новым токеном). Ошибок больше нет.
async fn on_text(
    ctx: &Ctx,
    out: &mpsc::Sender<MarketEvent>,
    s: &str,
    seen: &mut HashSet<String>,
) -> Result<bool, String> {
    match depth::parse(s) {
        Msg::Ack {
            success: false,
            method,
            error,
            ..
        } if method == "subscribe" => Err(format!(
            "подписка balances отклонена Kraken: {}",
            error.unwrap_or_default()
        )),
        Msg::Balances { kind, data } if kind == "update" => {
            for row in data.as_array().map(Vec::as_slice).unwrap_or_default() {
                match super::balance_event::parse(row, &ctx.venue(), seen) {
                    Err(why) => {
                        let m = format!("Проводка balances не разобрана: {why}");
                        ctx.log(Level::Error, "venue.user_stream", m).await;
                    }
                    Ok((_, _, None)) => {}
                    Ok((kind, ledger, Some(c))) => {
                        let m = format!(
                            "Проводка {ledger} ({kind}): {} {:+}",
                            c.asset.ticker, c.delta
                        );
                        ctx.log(Level::Info, "venue.user_stream", m).await;
                        if out.send(MarketEvent::Balance(c)).await.is_err() {
                            return Ok(false);
                        }
                    }
                }
            }
            Ok(true)
        }
        _ => Ok(true),
    }
}

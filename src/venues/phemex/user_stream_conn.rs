//! Файл: venues/phemex/user_stream_conn.rs
//! Что делает: одна сессия потока аккаунта Phemex: подключение к venue.ws_url, вход user.auth (sign.rs), подписка
//! wo.subscribe, свой server.ping раз в venue.ws_ping_ms, тишина дольше venue.ws_silence_ms — обрыв; сырые кадры — в
//! журнал (data/raw/phemex, соединение «account»; запрос входа с ключом не пишется); изменения баланса
//! (wallet_update.rs) → журнал и события ядру MarketEvent::Balance.
//! Кто вызывает: user_stream.rs.
//! От чего зависит: ws.rs (config_ping), depth.rs (ответы), wallet_update.rs, sign.rs, symbols.rs, ctx.rs.
//! Настройки: venue.ws_url, venue.recv_window_ms (срок expiry входа, не больше 120 000 мс) [phemex]; сроки — ws.rs.
//!
//! Официально (Spot Websocket API, https://phemex-docs.github.io/#user-authentication-2 и #subscribe-wallet-order):
//! {"method": "user.auth", "params": ["API", ключ, подпись, expiry], "id": …} → {"error": null, "id": …, "result":
//! {"status": "success"}}; expiry — «Maximum expiry is request time plus 2 minutes»; {"id": …, "method":
//! "wo.subscribe", "params": []} — «0 or more latest WO snapshot messages will be sent … and incremental messages will be
//! sent for later updates».

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
use super::symbols::Symbols;
use super::wallet_update::{self, Frame, Seen};

/// Имя соединения в сыром журнале.
const NAME: &str = "account";
/// Наибольший срок входа — «Maximum expiry is request time plus 2 minutes».
const AUTH_MAX_MS: u64 = 120_000;
/// id запросов сессии: вход и подписка (0 — ping).
const AUTH_ID: u64 = 1;
const SUB_ID: u64 = 2;

/// Настройки сессии.
pub struct Cfg {
    pub url: String,
    pub ping: Duration,
    pub silence: Duration,
    pub timeout: Duration,
    /// Срок expiry входа от «сейчас», секунды.
    pub expiry_s: i64,
}

/// Читает настройки. Ошибка — текст «не задана …» или «вне пределов документации».
pub fn config(ctx: &Ctx) -> Result<Cfg, String> {
    let (ping, silence) = super::ws::config_ping(ctx)?;
    let window = ctx.int("venue.recv_window_ms").map_err(|e| e.message)?;
    if window > AUTH_MAX_MS {
        return Err(format!(
            "venue.recv_window_ms = {window}: вход WebSocket Phemex — expiry не дальше 2 минут ({AUTH_MAX_MS} мс)"
        ));
    }
    Ok(Cfg {
        url: ctx.text("venue.ws_url").map_err(|e| e.message)?,
        ping,
        silence,
        timeout: ctx.http_timeout().map_err(|e| e.message)?,
        expiry_s: window.div_ceil(1000) as i64,
    })
}

/// Сессия до обрыва. Принимает: окружение, справочник (масштабы), канал ядра, настройки. Возвращает: Some(причина) —
/// переподключиться; None — канал ядра закрыт.
pub async fn session(
    ctx: &Ctx,
    symbols: &Symbols,
    out: &mpsc::Sender<MarketEvent>,
    cfg: &Cfg,
) -> Option<String> {
    if let Err(e) = symbols.ensure_fresh(ctx).await {
        return Some(format!("справочник монет (масштабы): {}", e.message));
    }
    let keys = match ctx.keys() {
        Ok(k) => k,
        Err(e) => return Some(e.message),
    };
    let ws = match tokio::time::timeout(
        cfg.timeout,
        tokio_tungstenite::connect_async(cfg.url.as_str()),
    )
    .await
    {
        Err(_) => return Some("не удалось подключиться за http.timeout_ms".into()),
        Ok(Err(e)) => return Some(format!("подключение: {e}")),
        Ok(Ok((ws, _))) => ws,
    };
    let auth = match super::sign::ws_auth(
        &keys,
        chrono::Utc::now().timestamp() + cfg.expiry_s,
        AUTH_ID,
    ) {
        Ok(a) => a,
        Err(e) => return Some(e.message),
    };
    drop(keys);
    let (mut sink, mut stream) = ws.split();
    if let Err(e) = sink.send(Message::Text(auth.to_string().into())).await {
        return Some(format!("отправка входа: {e}"));
    }
    // Ответ на вход: до него подписка не принимается.
    let answer = tokio::time::timeout(cfg.timeout, async {
        while let Some(Ok(f)) = stream.next().await {
            if let Message::Text(s) = f
                && let Msg::Reply {
                    id: Some(AUTH_ID),
                    ok,
                    text,
                } = depth::parse(&s)
            {
                return if ok { Ok(()) } else { Err(text) };
            }
        }
        Err("соединение закрыто до ответа на вход".into())
    });
    match answer.await {
        Ok(Ok(())) => {}
        Ok(Err(t)) => return Some(format!("вход не принят: {t}")),
        Err(_) => return Some("нет ответа на вход за http.timeout_ms".into()),
    }
    let sub = json!({"id": SUB_ID, "method": "wo.subscribe", "params": []});
    if let Err(e) = sink.send(Message::Text(sub.to_string().into())).await {
        return Some(format!("отправка wo.subscribe: {e}"));
    }
    let ping_msg = json!({"id": 0, "method": "server.ping", "params": []}).to_string();
    let mut ping = tokio::time::interval_at(Instant::now() + cfg.ping, cfg.ping);
    let mut last_rx = Instant::now();
    let mut seen = Seen::new();
    loop {
        tokio::select! {
            _ = ping.tick() => {
                if let Err(e) = sink.send(Message::Text(ping_msg.clone().into())).await {
                    return Some(format!("отправка server.ping: {e}"));
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
                last_rx = Instant::now();
                match frame {
                    Message::Text(s) => {
                        ctx.raw.write(ID, NAME, &s);
                        if let Msg::Reply { ok: false, text, .. } = depth::parse(&s) {
                            return Some(format!("ошибка Phemex: {text}"));
                        }
                        if !on_text(ctx, symbols, out, &s, &mut seen).await {
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

/// Кадр wallet-order → журнал и события ядру. Возвращает: false — канал ядра закрыт. Ошибок нет.
async fn on_text(
    ctx: &Ctx,
    symbols: &Symbols,
    out: &mpsc::Sender<MarketEvent>,
    s: &str,
    seen: &mut Seen,
) -> bool {
    match wallet_update::parse(s, &ctx.venue(), &symbols.scales(), seen) {
        Frame::Bad(why) => {
            let m = format!("Поток аккаунта: кадр wallet-order не разобран: {why}");
            ctx.log(Level::Error, "venue.user_stream", m).await;
            true
        }
        Frame::Update { changes, .. } => {
            for c in changes {
                if out.send(MarketEvent::Balance(c)).await.is_err() {
                    return false;
                }
            }
            true
        }
        Frame::Other => true,
    }
}

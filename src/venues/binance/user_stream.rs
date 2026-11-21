//! Файл: venues/binance/user_stream.rs
//! Что делает: поток аккаунта Binance (решение 76, урок 16) — изменения баланса для зачисления переводов без опроса.
//! Жизненный цикл: включено venue.balance_events [binance] → сессия WebSocket API с подпиской
//! userDataStream.subscribe.signature (user_stream_conn.rs) → при обрыве пауза venue.ws_reconnect_pause_ms и заново;
//! изменение настроек — сессия заново. Выключено или настройка не задана — потока нет, одна запись в журнал, ждём
//! изменения настроек. Хеша транзакции в событии нет: закрывать им зачисление или нет — настройка ядра
//! transfer.credit_by_event [binance] (до проверки на живой записи не включать).
//! Кто вызывает: subs.rs (с первой подпиской на рынок, живёт, пока жив канал ядра).
//! От чего зависит: user_stream_conn.rs, ctx.rs, settings/live.rs (Watch).
//! Настройки: venue.balance_events, venue.ws_private_url, venue.ws_silence_ms, venue.ws_reconnect_pause_ms,
//! venue.recv_window_ms [binance], http.timeout_ms.
//!
//! Официально: listenKey устарел (CHANGELOG 2025-04-07 «deprecated»); путь — WebSocket API
//! wss://ws-api.binance.com:443/ws-api/v3 и userDataStream.subscribe.signature (apiKey, timestamp, signature) — «In any
//! session, authenticated or not, you can subscribe to events for one or more accounts for which you can provide an
//! API Key signature» (websocket-api «User Data Stream requests»); соединение живёт 24 ч, ping сервера раз в 20 с.

use std::time::Duration;

use tokio::sync::mpsc;

use crate::gateway::events::MarketEvent;
use crate::settings::live::Watch;
use crate::storage::journal::Level;

use super::ctx::Ctx;

/// Настройки сессии.
pub struct Cfg {
    /// Адрес WebSocket API (venue.ws_private_url).
    pub url: String,
    pub silence: Duration,
    pub connect_timeout: Duration,
}

/// Работает, пока жив канал ядра. Ошибок не возвращает: всё — в журнал.
pub async fn run(ctx: Ctx, out: mpsc::Sender<MarketEvent>) {
    let names = [
        "venue.balance_events",
        "venue.ws_private_url",
        "venue.ws_silence_ms",
        "venue.recv_window_ms",
        "http.timeout_ms",
    ];
    let mut changes = Watch::new(
        ctx.settings.subscribe(),
        names.iter().map(|n| n.to_string()).collect(),
    );
    let mut told = String::new();
    while !out.is_closed() {
        let (why, retry) = match ctx.flag("venue.balance_events") {
            Ok(true) => match config(&ctx) {
                Ok(cfg) => tokio::select! {
                    w = super::user_stream_conn::session(&ctx, &out, &cfg) => match w {
                        Some(w) => (w, true),
                        None => return,
                    },
                    c = changes.next() => match c {
                        None => return,
                        Some(c) => (format!("{} изменена — поток аккаунта заново", c.name), true),
                    },
                },
                Err(e) => (e, false),
            },
            Ok(false) => (
                "venue.balance_events = нет — поток аккаунта выключен".into(),
                false,
            ),
            Err(e) => (e.message, false),
        };
        if why != told {
            let level = if retry { Level::Warn } else { Level::Info };
            ctx.log(level, "venue.user_stream", format!("Поток аккаунта: {why}"))
                .await;
            told = why;
        }
        let pause = ctx
            .int("venue.ws_reconnect_pause_ms")
            .ok()
            .filter(|_| retry);
        let wait = async {
            match pause {
                Some(ms) => tokio::time::sleep(Duration::from_millis(ms)).await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            _ = wait => {}
            c = changes.next() => if c.is_none() { return },
        }
    }
}

/// Настройки сессии. Ошибка — текст: не задана.
fn config(ctx: &Ctx) -> Result<Cfg, String> {
    Ok(Cfg {
        url: ctx.text("venue.ws_private_url").map_err(|e| e.message)?,
        silence: Duration::from_millis(ctx.int("venue.ws_silence_ms").map_err(|e| e.message)?),
        connect_timeout: ctx.http_timeout().map_err(|e| e.message)?,
    })
}

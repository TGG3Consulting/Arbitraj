//! Файл: venues/bybit/user_stream.rs
//! Что делает: жизненный цикл приватного потока Bybit (order.spot, execution.spot, wallet): включено
//! venue.private_stream [bybit] → сессия (user_stream_conn.rs) → при обрыве пауза venue.ws_reconnect_pause_ms и
//! переподключение; изменение настроек — сессия заново. Выключено или настройка не задана — потока нет, одна запись в
//! журнал, ждём изменения настроек. Без потока итог ордера читается опросом REST (order_state.rs) — так же верно, но
//! позже.
//! Изменения баланса ядру (MarketEvent::Balance, решение 76) Bybit НЕ шлёт: депозит зачисляется на FUND (Set Deposit
//! Account: «deposited into FUND wallet by default»), а канал wallet описывает только UNIFIED («accountType UNIFIED»), и
//! в нём нет причины изменения и хеша — зачисление перевода закрывает опрос статуса депозита (deposits.rs).
//! Кто вызывает: subs.rs (с первой подпиской на рынок, живёт, пока жив канал ядра).
//! От чего зависит: user_stream_conn.rs, ctx.rs, settings/live.rs (Watch).
//! Настройки: venue.private_stream, venue.ws_private_url, venue.ws_silence_ms, venue.ws_ping_ms,
//! venue.ws_reconnect_pause_ms, venue.recv_window_ms [bybit], http.timeout_ms.
//!
//! Официально (Connect https://bybit-exchange.github.io/docs/v5/ws/connect): приватный поток —
//! wss://stream.bybit.com/v5/private; «if there is no "ping-pong" and no stream data sent from server end, the connection
//! will be cut off after 10 minutes»; ping советуют раз в 20 с.

use std::time::Duration;

use tokio::sync::mpsc;

use crate::gateway::events::MarketEvent;
use crate::settings::live::Watch;
use crate::storage::journal::Level;

use super::ctx::{Ctx, ID};

/// Настройки сессии.
pub struct Cfg {
    pub url: String,
    pub silence: Duration,
    pub ping: Duration,
    pub connect_timeout: Duration,
    /// Срок подписи auth (expires = сейчас + venue.recv_window_ms).
    pub sign_ms: i64,
}

/// Работает, пока жив канал ядра. Ошибок не возвращает: всё — в журнал.
pub async fn run(ctx: Ctx, out: mpsc::Sender<MarketEvent>) {
    let names = [
        "venue.private_stream",
        "venue.ws_private_url",
        "venue.ws_silence_ms",
        "venue.ws_ping_ms",
        "venue.recv_window_ms",
        "http.timeout_ms",
    ];
    let mut changes = Watch::new(
        ctx.settings.subscribe(),
        names.iter().map(|n| n.to_string()).collect(),
    );
    let mut told = String::new();
    while !out.is_closed() {
        let (why, retry) = match ctx.flag("venue.private_stream") {
            Ok(true) => match config(&ctx) {
                Ok(cfg) => tokio::select! {
                    w = super::user_stream_conn::session(&ctx, &cfg) => (w, true),
                    c = changes.next() => match c {
                        None => return,
                        Some(c) => (format!("{} изменена — приватный поток заново", c.name), true),
                    },
                },
                Err(e) => (e, false),
            },
            Ok(false) => ("venue.private_stream = нет — приватный поток выключен (итог ордеров — опросом REST)".into(), false),
            Err(e) => (e.message, false),
        };
        if why != told {
            let level = if retry { Level::Warn } else { Level::Info };
            let m = format!("Приватный поток: {why}");
            let _ = ctx
                .journal
                .write(level, "venue.user_stream", Some(ID), m, None)
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

/// Настройки сессии. Ошибка — текст «не задана …».
fn config(ctx: &Ctx) -> Result<Cfg, String> {
    Ok(Cfg {
        url: ctx.text("venue.ws_private_url").map_err(|e| e.message)?,
        silence: Duration::from_millis(ctx.int("venue.ws_silence_ms").map_err(|e| e.message)?),
        ping: Duration::from_millis(ctx.int("venue.ws_ping_ms").map_err(|e| e.message)?),
        connect_timeout: ctx.http_timeout().map_err(|e| e.message)?,
        sign_ms: ctx.int("venue.recv_window_ms").map_err(|e| e.message)? as i64,
    })
}

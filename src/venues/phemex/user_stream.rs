//! Файл: venues/phemex/user_stream.rs
//! Что делает: поток аккаунта Phemex (решение 76, урок 16) — изменения баланса спота (канал wallet-order) для
//! зачисления переводов без опроса. Жизненный цикл: включено venue.balance_events [phemex] → сессия
//! (user_stream_conn.rs: вход user.auth, подписка wo.subscribe) → при обрыве пауза venue.ws_reconnect_pause_ms и
//! заново; изменение настроек — сессия заново. Выключено или настройка не задана — потока нет, одна запись в журнал,
//! ждём изменения настроек.
//! Кто вызывает: subs.rs (с первой подпиской на рынок, живёт, пока жив канал ядра).
//! От чего зависит: user_stream_conn.rs, symbols.rs, ctx.rs, settings/live.rs (Watch).
//! Настройки: venue.balance_events, venue.ws_url, venue.ws_ping_ms, venue.ws_silence_ms, venue.recv_window_ms,
//! venue.ws_reconnect_pause_ms [phemex], http.timeout_ms.
//!
//! Официально (Spot Websocket API, https://phemex-docs.github.io/#user-authentication-2 и #subscribe-wallet-order):
//! приватные каналы — на том же адресе, что публичные, после user.auth; «WO subscription requires the session been
//! authorized successfully». Соединение потока считается в пределе «5 соединений на клиента» (market.rs).

use std::time::Duration;

use tokio::sync::mpsc;

use crate::gateway::events::MarketEvent;
use crate::settings::live::Watch;
use crate::storage::journal::Level;

use super::ctx::Ctx;
use super::symbols::Symbols;

/// Работает, пока жив канал ядра. Ошибок не возвращает: всё — в журнал.
pub async fn run(ctx: Ctx, symbols: Symbols, out: mpsc::Sender<MarketEvent>) {
    let names = [
        "venue.balance_events",
        "venue.ws_url",
        "venue.ws_ping_ms",
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
            Ok(true) => match super::user_stream_conn::config(&ctx) {
                Ok(cfg) => tokio::select! {
                    w = super::user_stream_conn::session(&ctx, &symbols, &out, &cfg) => match w {
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

//! Файл: venues/kraken/user_stream.rs
//! Что делает: поток аккаунта Kraken (решение 76, урок 16) — депозиты и выводы спота событиями канала balances без
//! опроса. Жизненный цикл: включено venue.balance_events [kraken] → сессия (user_stream_conn.rs: токен, подписка) → при
//! обрыве пауза venue.ws_reconnect_pause_ms и заново; изменение настроек — сессия заново. Выключено или настройка не
//! задана — потока нет, одна запись в журнал, ждём изменения настроек.
//! Кто вызывает: subs.rs (с первой подпиской на рынок, живёт, пока жив канал ядра).
//! От чего зависит: user_stream_conn.rs, ws.rs (config_ping), ctx.rs, settings/live.rs (Watch).
//! Настройки: venue.balance_events, venue.ws_private_url, venue.ws_ping_ms, venue.ws_silence_ms,
//! venue.ws_reconnect_pause_ms [kraken], http.timeout_ms.
//!
//! Официально: канал balances — на wss://ws-auth.kraken.com/v2 (https://docs.kraken.com/exchange/api-reference/
//! spot-websocket-v2/balances.md; только настройка venue.ws_private_url), подписка с токеном GetWebSocketsToken
//! (https://docs.kraken.com/exchange/guides/websockets/authentication.md).

use std::collections::HashSet;
use std::time::Duration;

use tokio::sync::mpsc;

use crate::gateway::events::MarketEvent;
use crate::settings::live::Watch;
use crate::storage::journal::Level;

use super::ctx::Ctx;

/// Работает, пока жив канал ядра. Ошибок не возвращает: всё — в журнал.
pub async fn run(ctx: Ctx, out: mpsc::Sender<MarketEvent>) {
    let names = [
        "venue.balance_events",
        "venue.ws_private_url",
        "venue.ws_ping_ms",
        "venue.ws_silence_ms",
        "http.timeout_ms",
    ];
    let mut changes = Watch::new(
        ctx.settings.subscribe(),
        names.iter().map(|n| n.to_string()).collect(),
    );
    let mut told = String::new();
    // Отданные ядру проводки (ledger_id) — на всё время процесса: у бота депозитов и выводов единицы в день.
    let mut seen: HashSet<String> = HashSet::new();
    while !out.is_closed() {
        let (why, retry) = match ctx.flag("venue.balance_events") {
            Ok(true) => match super::user_stream_conn::config(&ctx) {
                Ok(cfg) => tokio::select! {
                    w = super::user_stream_conn::session(&ctx, &out, &cfg, &mut seen) => match w {
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

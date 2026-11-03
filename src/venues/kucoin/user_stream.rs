//! Файл: venues/kucoin/user_stream.rs
//! Что делает: поток аккаунта KuCoin (решение 76, урок 16) — изменения баланса (/account/balance) для зачисления
//! переводов без опроса и события ордеров (/spotMarket/tradeOrdersV2 — в сырой журнал). Жизненный цикл: включено
//! venue.balance_events [kucoin] → приватный токен (bullet-private, новый на каждое подключение) → сессия
//! (user_stream_conn.rs) → при обрыве пауза venue.ws_reconnect_pause_ms и заново. Выключено или настройка не задана —
//! потока нет, одна запись в журнал, ждём изменения настроек.
//! Кто вызывает: subs.rs (с первой подпиской на рынок, живёт, пока жив канал ядра).
//! От чего зависит: bullet.rs, user_stream_conn.rs, ctx.rs, settings/live.rs (Watch).
//! Настройки: venue.balance_events, venue.ws_reconnect_pause_ms [kucoin], http.timeout_ms.
//!
//! Официально: Get Private Token POST /api/v1/bullet-private (подписанный)
//! https://www.kucoin.com/docs-new/websocket-api/base-info/get-private-token-spot-margin; токен действует 24 часа —
//! после разрыва берётся новый.

use std::time::Duration;

use tokio::sync::mpsc;

use crate::gateway::events::MarketEvent;
use crate::settings::live::Watch;
use crate::storage::journal::Level;

use super::ctx::Ctx;

/// Работает, пока жив канал ядра. Ошибок не возвращает: всё — в журнал.
pub async fn run(ctx: Ctx, out: mpsc::Sender<MarketEvent>) {
    let names = ["venue.balance_events", "http.timeout_ms"];
    let mut changes = Watch::new(
        ctx.settings.subscribe(),
        names.iter().map(|n| n.to_string()).collect(),
    );
    let mut told = String::new();
    while !out.is_closed() {
        let (why, retry) = match ctx.flag("venue.balance_events") {
            Ok(true) => match (ctx.http_timeout(), super::bullet::get(&ctx, true).await) {
                (Ok(t), Ok(token)) => {
                    tokio::select! {
                        why = super::user_stream_conn::session(&ctx, &out, &token, t) => match why {
                            Some(w) => (w, true),
                            None => return,
                        },
                        c = changes.next() => match c {
                            None => return,
                            Some(c) => (format!("{} изменена — поток аккаунта заново", c.name), true),
                        },
                    }
                }
                (Err(e), _) => (e.message, false),
                (_, Err(e)) => (
                    format!("токен bullet-private не получен: {}", e.message),
                    true,
                ),
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

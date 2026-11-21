//! Файл: venues/okx/user_stream.rs
//! Что делает: поток аккаунта OKX (решение 76, урок 16) — зачисления депозитов событиями (канал deposit-info, с хешем
//! транзакции) без опроса. Жизненный цикл: включено venue.balance_events [okx] → сессия (user_stream_conn.rs: вход,
//! подписка) → при обрыве пауза venue.ws_reconnect_pause_ms и заново; изменение настроек — сессия заново. Выключено или
//! настройка не задана — потока нет, одна запись в журнал, ждём изменения настроек.
//! Кто вызывает: subs.rs (с первой подпиской на рынок, живёт, пока жив канал ядра).
//! От чего зависит: user_stream_conn.rs, ws.rs (config_ping), ctx.rs, settings/live.rs (Watch).
//! Настройки: venue.balance_events, venue.ws_business_url, venue.ws_ping_ms, venue.ws_silence_ms,
//! venue.ws_reconnect_pause_ms [okx], http.timeout_ms.
//!
//! Официально (https://www.okx.com/docs-v5/en/, «Overview → WebSocket»): канал deposit-info — на адресе business
//! (wss://ws.okx.com/ws/v5/business у основного домена; у другой сущности — свой, только настройка), «Login required».
//! Каналы торгового счёта account / balance_and_position (адрес private) причину изменения «депозит» не сообщают
//! (eventType: snapshot, delivered, exercised, transferred, filled, …) — для зачисления не годятся и не слушаются.

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
        "venue.ws_business_url",
        "venue.ws_ping_ms",
        "venue.ws_silence_ms",
        "http.timeout_ms",
    ];
    let mut changes = Watch::new(
        ctx.settings.subscribe(),
        names.iter().map(|n| n.to_string()).collect(),
    );
    let mut told = String::new();
    // Отданные ядру депозиты (depId) — на всё время процесса: у бота депозитов единицы в день.
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

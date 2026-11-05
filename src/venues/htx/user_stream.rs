//! Файл: venues/htx/user_stream.rs
//! Что делает: поток аккаунта HTX (решение 76, урок 16) — изменения баланса спот-счёта (accounts.update#2) для
//! зачисления переводов без опроса. Жизненный цикл: включено venue.balance_events [htx] → id спот-счёта → сессия
//! (user_stream_conn.rs: вход подписью, подписка) → при обрыве пауза venue.ws_reconnect_pause_ms и заново. Выключено
//! или настройка не задана — потока нет, одна запись в журнал, ждём изменения настроек.
//! Кто вызывает: subs.rs (с первой подпиской на рынок, живёт, пока жив канал ядра).
//! От чего зависит: user_stream_conn.rs, accounts.rs, ctx.rs, settings/live.rs (Watch).
//! Настройки: venue.balance_events, venue.ws_private_url, venue.ws_silence_ms, venue.ws_reconnect_pause_ms [htx],
//! http.timeout_ms.
//!
//! Официально («Websocket Account and Order → Introduction», https://huobiapi.github.io/docs/spot/v1/en/): адрес
//! «wss://api.huobi.pro/ws/v2» (или api-aws) — настройка venue.ws_private_url; сервер шлёт ping примерно раз в 20 с —
//! venue.ws_silence_ms должна быть больше (проверяется); «10 connections per API key» — у бота одно соединение.

use std::time::Duration;

use tokio::sync::mpsc;

use crate::gateway::events::MarketEvent;
use crate::settings::live::Watch;
use crate::storage::journal::Level;

use super::accounts::SpotId;
use super::ctx::Ctx;
use super::user_stream_conn::{self, Cfg};

/// Интервал ping сервера потока аккаунта, мс — «about every 20s» (тишина должна быть больше).
pub const SERVER_PING_MS: u64 = 20_000;

/// Работает, пока жив канал ядра. Ошибок не возвращает: всё — в журнал.
pub async fn run(ctx: Ctx, spot: SpotId, out: mpsc::Sender<MarketEvent>) {
    let names = [
        "venue.balance_events",
        "venue.ws_private_url",
        "venue.ws_silence_ms",
        "http.timeout_ms",
    ];
    let mut changes = Watch::new(
        ctx.settings.subscribe(),
        names.iter().map(|n| n.to_string()).collect(),
    );
    let mut told = String::new();
    while !out.is_closed() {
        let (why, retry) = match ctx.flag("venue.balance_events") {
            Ok(true) => match (config(&ctx), spot.require(&ctx).await) {
                (Ok(cfg), Ok(id)) => {
                    tokio::select! {
                        why = user_stream_conn::session(&ctx, &out, id, cfg) => match why {
                            Some(w) => (w, true),
                            None => return,
                        },
                        c = changes.next() => match c {
                            None => return,
                            Some(c) => (format!("{} изменена — поток аккаунта заново", c.name), true),
                        },
                    }
                }
                (Err(m), _) => (m, false),
                (_, Err(e)) => (format!("id спот-счёта не получен: {}", e.message), true),
            },
            Ok(false) => (
                "venue.balance_events = нет — поток аккаунта выключен".into(),
                false,
            ),
            Err(e) => (e.message, false),
        };
        if why != told {
            let level = if retry { Level::Warn } else { Level::Info };
            ctx.log(
                level,
                "venue.user_stream",
                format!("Поток аккаунта HTX: {why}"),
            )
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

/// Настройки сессии. Ошибка — текст причины (не задана; тишина не больше ping сервера).
fn config(ctx: &Ctx) -> Result<Cfg, String> {
    let m = |e: crate::gateway::errors::GatewayError| e.message;
    let silence = ctx.int("venue.ws_silence_ms").map_err(m)?;
    if silence <= SERVER_PING_MS {
        return Err(format!(
            "venue.ws_silence_ms = {silence}: поток аккаунта HTX получает ping раз в ~{SERVER_PING_MS} мс — нужно больше"
        ));
    }
    Ok(Cfg {
        url: ctx.text("venue.ws_private_url").map_err(m)?,
        timeout: ctx.http_timeout().map_err(m)?,
        silence: Duration::from_millis(silence),
    })
}

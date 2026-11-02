//! Файл: venues/mexc/user_stream.rs
//! Что делает: поток аккаунта MEXC — изменения баланса (канал spot@private.account.v3.api.pb) для зачисления переводов
//! без опроса статуса. Жизненный цикл: включено venue.balance_events [mexc] → новый listenKey (user_key.rs) → сессия
//! (user_stream_conn.rs) → при обрыве ключ закрывается, пауза venue.ws_reconnect_pause_ms, новый ключ. Выключено или
//! настройка не задана — потока нет, одна запись в журнал, ждём изменения настроек.
//! Хеша транзакции в событии MEXC нет: закрывать им зачисление или нет — настройка ядра transfer.credit_by_event [mexc]
//! (DECISIONS 76, 107: до проверки на живой записи не включать).
//! Кто вызывает: subs.rs (запускается один раз с первой подпиской на рынок, живёт, пока жив канал ядра).
//! От чего зависит: user_key.rs, user_stream_conn.rs, ctx.rs, settings/live.rs (Watch).
//! Настройки: venue.balance_events, venue.ws_url, venue.user_stream_keepalive_s, venue.ws_silence_ms,
//! venue.ws_reconnect_pause_ms, venue.ws_ping_ms [mexc], http.timeout_ms.
//!
//! Официально (https://www.mexc.com/api-docs/spot-v3/websocket-user-data-streams/): адрес — тот же, что у рынка
//! (venue.ws_url), с «?listenKey=»; ключ живёт 60 минут, продлевать раз в 30 минут; соединение живёт не больше 24 часов.

use std::time::Duration;

use tokio::sync::mpsc;

use crate::gateway::events::MarketEvent;
use crate::settings::live::Watch;
use crate::storage::journal::Level;

use super::ctx::{Ctx, ID};
use super::user_key::{self, KEY_LIFETIME_S};

/// Настройки сессии.
pub struct Cfg {
    /// Адрес без ключа (venue.ws_url).
    pub url: String,
    pub keepalive: Duration,
    pub silence: Duration,
    pub connect_timeout: Duration,
    /// Как часто слать {"method":"PING"} (venue.ws_ping_ms).
    pub ping: Duration,
}

/// Работает, пока жив канал ядра. Ошибок не возвращает: всё — в журнал.
pub async fn run(ctx: Ctx, out: mpsc::Sender<MarketEvent>) {
    let names = [
        "venue.balance_events",
        "venue.ws_url",
        "venue.user_stream_keepalive_s",
        "venue.ws_silence_ms",
        "http.timeout_ms",
        "venue.ws_ping_ms",
    ];
    let mut changes = Watch::new(
        ctx.settings.subscribe(),
        names.iter().map(|n| n.to_string()).collect(),
    );
    let mut told = String::new();
    while !out.is_closed() {
        let (why, retry) = match ctx.flag("venue.balance_events") {
            Ok(true) => match config(&ctx) {
                Ok(cfg) => match user_key::create(&ctx).await {
                    Ok(key) => {
                        let why = tokio::select! {
                            why = super::user_stream_conn::session(&ctx, &out, &cfg, &key) => why,
                            c = changes.next() => match c {
                                None => return,
                                Some(c) => Some(format!("{} изменена — поток аккаунта заново", c.name)),
                            },
                        };
                        user_key::close(&ctx, &key).await;
                        match why {
                            None => return,
                            Some(w) => (w, true),
                        }
                    }
                    Err(e) => (format!("listenKey не получен: {}", e.message), true),
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
            let _ = ctx
                .journal
                .write(
                    level,
                    "venue.user_stream",
                    Some(ID),
                    format!("Поток аккаунта: {why}"),
                    None,
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

/// Настройки сессии. Ошибка — текст: не задана или продление реже срока жизни ключа (1 час по документации).
fn config(ctx: &Ctx) -> Result<Cfg, String> {
    let keepalive = ctx
        .int("venue.user_stream_keepalive_s")
        .map_err(|e| e.message)?;
    if keepalive >= KEY_LIFETIME_S {
        return Err(format!(
            "venue.user_stream_keepalive_s = {keepalive}: listenKey MEXC живёт {KEY_LIFETIME_S} с — продлевать нужно чаще"
        ));
    }
    Ok(Cfg {
        url: ctx.text("venue.ws_url").map_err(|e| e.message)?,
        keepalive: Duration::from_secs(keepalive),
        silence: Duration::from_millis(ctx.int("venue.ws_silence_ms").map_err(|e| e.message)?),
        connect_timeout: ctx.http_timeout().map_err(|e| e.message)?,
        ping: Duration::from_millis(ctx.int("venue.ws_ping_ms").map_err(|e| e.message)?),
    })
}

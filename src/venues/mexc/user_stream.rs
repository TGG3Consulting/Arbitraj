//! Файл: venues/mexc/user_stream.rs
//! Что делает: поток аккаунта MEXC — изменения баланса (канал spot@private.account.v3.api.pb) для зачисления переводов
//! без опроса статуса. Жизненный цикл: включено venue.balance_events [mexc] → новый listenKey (user_key.rs) → сессия
//! (user_stream_conn.rs) → при обрыве пауза venue.ws_reconnect_pause_ms и переподключение с тем же ключом, если его
//! продление проходит; новый ключ — только при отказе продления (старый закрывается) — решение 118, А-🟠10: не больше
//! 60 ключей на UID. Изменение настроек и конец работы — ключ закрывается. Выключено или настройка не задана — потока
//! нет, одна запись в журнал, ждём изменения настроек.
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
use zeroize::Zeroizing;

use crate::gateway::errors::GatewayResult;
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

/// Чем кончилась сессия.
enum End {
    /// Обрыв соединения (причина) — ключ можно переиспользовать.
    Broken(String),
    /// Изменилась настройка — ключ закрывается.
    Changed(String),
    /// Канал ядра закрыт — конец работы.
    Closed,
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
    // Ключ, переживший обрыв сессии: переиспользуется, пока продление проходит.
    let mut kept: Option<Zeroizing<String>> = None;
    while !out.is_closed() {
        let (why, retry) = match ctx.flag("venue.balance_events") {
            Ok(true) => match config(&ctx) {
                Ok(cfg) => match key(&ctx, kept.take()).await {
                    Ok(key) => {
                        let end = tokio::select! {
                            why = super::user_stream_conn::session(&ctx, &out, &cfg, &key) => match why {
                                Some(w) => End::Broken(w),
                                None => End::Closed,
                            },
                            c = changes.next() => match c {
                                None => End::Closed,
                                Some(c) => End::Changed(format!("{} изменена — поток аккаунта заново", c.name)),
                            },
                        };
                        match end {
                            // Обрыв соединения: ключ остаётся (его проверит продление в начале круга).
                            End::Broken(w) => {
                                kept = Some(key);
                                (w, true)
                            }
                            End::Changed(w) => {
                                user_key::close(&ctx, &key).await;
                                (w, true)
                            }
                            End::Closed => {
                                user_key::close(&ctx, &key).await;
                                return;
                            }
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
        if !retry && let Some(k) = kept.take() {
            user_key::close(&ctx, &k).await;
        }
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

/// Что делает: ключ для сессии — прежний, если продление прошло; иначе прежний закрывается и берётся новый.
/// Принимает: окружение, прежний ключ. Возвращает: ключ. Ошибки: новый ключ не получен.
async fn key(ctx: &Ctx, kept: Option<Zeroizing<String>>) -> GatewayResult<Zeroizing<String>> {
    if let Some(k) = kept {
        if user_key::extend(ctx, &k).await.is_ok() {
            return Ok(k);
        }
        user_key::close(ctx, &k).await;
    }
    user_key::create(ctx).await
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

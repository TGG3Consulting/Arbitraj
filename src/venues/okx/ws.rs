//! Файл: venues/okx/ws.rs
//! Что делает: жизненный цикл одного соединения WebSocket стаканов OKX: настройки, сессия (ws_conn.rs), при обрыве — все
//! стаканы соединения недостоверны, журнал, пауза, переподключение. Нет нужной настройки — соединение останавливается
//! (без бесконечных повторов).
//! Кто вызывает: market.rs (по задаче на соединение), user_stream.rs (config_ping — те же сроки пинга).
//! От чего зависит: ws_conn.rs, book.rs, ctx.rs.
//! Настройки: venue.ws_url, venue.depth_levels (5 → books5, 400 → books), venue.ws_ping_ms, venue.ws_silence_ms,
//! venue.ws_reconnect_pause_ms [okx], http.timeout_ms (время на подключение).
//!
//! Официально (https://www.okx.com/docs-v5/en/, «Overview → WebSocket → Connect»): адрес публичных каналов —
//! wss://ws.okx.com/ws/v5/public (у другой сущности счёта — свой; только настройка); «The connection will break
//! automatically if the subscription is not established or data has not been pushed for more than 30 seconds»; «Set a
//! timer of N seconds whenever a response message is received, where N is less than 30. If the timer is triggered …
//! send the String 'ping'. Expect a 'pong' as a response. If the response message is not received within N seconds,
//! please raise an error or reconnect». «Order book channel»: books5 — 5 уровней, books — 400 уровней.

use std::time::Duration;

use tokio::sync::mpsc;

use crate::core::types::Asset;
use crate::gateway::events::MarketEvent;
use crate::storage::journal::Level;

use super::book::Books;
use super::ctx::Ctx;
use super::ws_conn;

/// Срок тишины сервера, после которого OKX рвёт соединение, мс — «more than 30 seconds»: пинг должен быть чаще.
pub const SERVER_SILENCE_MS: u64 = 30_000;

/// Почему сессия закончилась.
pub enum End {
    /// Канал ядра закрыт — работа окончена.
    Done,
    /// Обрыв — переподключиться.
    Broken(String),
}

/// Настройки соединения.
pub struct Cfg {
    pub url: String,
    /// Канал стакана: books5 или books.
    pub channel: &'static str,
    /// Сколько уровней на сторону отдавать ядру (5 или 400).
    pub levels: usize,
    pub ping: Duration,
    pub silence: Duration,
    pub connect_timeout: Duration,
    /// Повторный разрыв той же пары раньше этого — переподключение, а не пересподписка (урок 12).
    pub resync_gap: Duration,
}

/// Работает, пока жив канал out: подключается заново после каждого обрыва.
/// Принимает: окружение, имя соединения, монеты (монета, instId), стаканы, канал ядра, сдвиг подключения (не больше
/// venue.ws_connects_per_sec подключений в секунду — market.rs).
pub async fn run(
    ctx: Ctx,
    name: String,
    assets: Vec<(Asset, String)>,
    books: Books,
    out: mpsc::Sender<MarketEvent>,
    delay: Duration,
) {
    tokio::time::sleep(delay).await;
    while !out.is_closed() {
        let end = match config(&ctx) {
            Ok(cfg) => ws_conn::session(&ctx, &name, &assets, &books, &out, cfg).await,
            Err(e) => {
                ctx.log(
                    Level::Error,
                    "venue.ws",
                    format!("WebSocket {name} остановлен: {e}"),
                )
                .await;
                return;
            }
        };
        let End::Broken(why) = end else { return };
        for (a, _) in &assets {
            invalidate(&ctx, &books, &out, &a.ticker);
        }
        let Ok(pause) = ctx.int("venue.ws_reconnect_pause_ms") else {
            let m = format!(
                "WebSocket {name} остановлен: {why}; не задана venue.ws_reconnect_pause_ms"
            );
            ctx.log(Level::Error, "venue.ws", m).await;
            return;
        };
        let m = format!("WebSocket {name}: {why}; переподключение");
        ctx.log(Level::Warn, "venue.ws", m).await;
        tokio::time::sleep(Duration::from_millis(pause) + delay).await;
    }
}

/// Объявляет стакан монеты недостоверным: убирает из памяти и сообщает ядру (без ожидания).
pub fn invalidate(ctx: &Ctx, books: &Books, out: &mpsc::Sender<MarketEvent>, ticker: &str) {
    books.drop_ticker(ticker);
    let _ = out.try_send(MarketEvent::Invalidated {
        venue: ctx.venue(),
        ticker: ticker.to_string(),
    });
}

/// Сроки пинга и тишины: (ping, silence). venue.ws_ping_ms < 30000 (сервер рвёт молчащее дольше 30 с).
/// Ошибка — текст «не задана …» или «вне пределов документации».
pub fn config_ping(ctx: &Ctx) -> Result<(Duration, Duration), String> {
    let ping = ctx.int("venue.ws_ping_ms").map_err(|e| e.message)?;
    if ping == 0 || ping >= SERVER_SILENCE_MS {
        return Err(format!(
            "venue.ws_ping_ms = {ping}: у OKX меньше {SERVER_SILENCE_MS} мс («N is less than 30»)"
        ));
    }
    let silence = ctx.int("venue.ws_silence_ms").map_err(|e| e.message)?;
    if silence <= ping {
        return Err(format!(
            "venue.ws_silence_ms = {silence} должна быть больше venue.ws_ping_ms = {ping} (ждём pong после ping)"
        ));
    }
    Ok((Duration::from_millis(ping), Duration::from_millis(silence)))
}

/// Читает настройки соединения. Ошибка — текст «не задана …» или «вне пределов документации».
pub fn config(ctx: &Ctx) -> Result<Cfg, String> {
    let (channel, levels) = match ctx.int("venue.depth_levels").map_err(|e| e.message)? {
        5 => ("books5", 5),
        400 => ("books", 400),
        other => {
            return Err(format!(
                "venue.depth_levels = {other}: у OKX только 5 (канал books5) или 400 (канал books)"
            ));
        }
    };
    let (ping, silence) = config_ping(ctx)?;
    let pause = ctx
        .int("venue.ws_reconnect_pause_ms")
        .map_err(|e| e.message)?;
    Ok(Cfg {
        url: ctx.text("venue.ws_url").map_err(|e| e.message)?,
        channel,
        levels,
        ping,
        silence,
        connect_timeout: ctx.http_timeout().map_err(|e| e.message)?,
        resync_gap: Duration::from_millis(pause),
    })
}

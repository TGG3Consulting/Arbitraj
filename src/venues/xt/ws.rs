//! Файл: venues/xt/ws.rs
//! Что делает: жизненный цикл одного соединения публичного WebSocket XT.com: чтение настроек (с пределами
//! документации), сессия (ws_conn.rs), при обрыве — все стаканы соединения недостоверны, журнал, пауза,
//! переподключение. Нет нужной настройки — соединение останавливается (без бесконечных повторов).
//! Кто вызывает: market.rs (по задаче на соединение).
//! От чего зависит: ws_conn.rs, snapshots.rs (config), book.rs, ctx.rs.
//! Настройки: venue.ws_url, venue.book_stream, venue.depth_levels, venue.ws_subs_per_sec, venue.ws_streams_per_msg,
//! venue.ws_ping_ms, venue.ws_silence_ms, venue.ws_reconnect_pause_ms [xt], http.timeout_ms; при depth_update —
//! venue.depth_snapshot_per_sec, venue.depth_snapshot_limit (snapshots.rs).
//!
//! Официально (https://doc.xt.com/#websocket_publicbase и далее, `_websocket_public/*.md`): адрес
//! wss://stream.xt.com/public; «Each link of the client needs to send a text "ping" periodically … If the server does not
//! receive a ping message from the client within 1 minute, it will actively disconnect» — ping чаще 60 с; уровни
//! снимка depth@{symbol},{levels}: 5, 10, 20, 50 (1000 мс); изменения depth_update@{symbol} — 100 мс. Лимиты подписок
//! на соединение, тем в запросе и сообщений в секунду в документации НЕ указаны — venue.ws_symbols_per_conn,
//! venue.ws_streams_per_msg, venue.ws_subs_per_sec подбираются на живом замере (решение 109).
//! Заголовок «Sec-Websocket-Extensions: permessage-deflate» документация требует, но tokio-tungstenite 0.30 сжатие
//! кадров не поддерживает: подключение без него (проверить на живом сервере — отчёт решения 167).

use std::time::Duration;

use tokio::sync::mpsc;

use crate::core::types::Asset;
use crate::gateway::events::MarketEvent;
use crate::storage::journal::Level;

use super::book::Books;
use super::ctx::Ctx;
use super::ws_conn;

/// Сервер рвёт соединение без ping дольше минуты — официально.
pub const SERVER_SILENCE_MS: u64 = 60_000;
/// Допустимые уровни снимка depth — официально 5, 10, 20, 50.
pub const DEPTH_LEVELS: [u64; 4] = [5, 10, 20, 50];

/// Почему сессия закончилась.
pub enum End {
    /// Канал ядра закрыт — работа окончена.
    Done,
    /// Обрыв — переподключиться.
    Broken(String),
    /// Нужная настройка не задана — соединение останавливается.
    Config(String),
}

/// Какой канал стакана слушать (venue.book_stream).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stream {
    /// depth@{symbol},{levels} — снимки раз в 1000 мс.
    Depth,
    /// depth_update@{symbol} + снимок REST — локальный стакан, изменения раз в 100 мс.
    Update,
}

/// Настройки соединения.
pub struct Cfg {
    pub url: String,
    pub stream: Stream,
    /// Уровни снимка (Depth) или сколько верхних уровней локального стакана отдавать ядру (Update).
    pub levels: u64,
    pub sub_gap: Duration,
    pub per_msg: usize,
    pub ping: Duration,
    pub silence: Duration,
    pub connect_timeout: Duration,
    /// Не чаще одного снимка пары за столько после разрыва (урок 12: без круга ресинков).
    pub resync_pause: Duration,
}

/// Работает, пока жив канал out: подключается заново после каждого обрыва.
/// Принимает: окружение, имя соединения, монеты (монета, символ XT), стаканы, канал ядра.
pub async fn run(
    ctx: Ctx,
    name: String,
    assets: Vec<(Asset, String)>,
    books: Books,
    out: mpsc::Sender<MarketEvent>,
) {
    while !out.is_closed() {
        let end = match config(&ctx) {
            Ok(cfg) => ws_conn::session(&ctx, &name, &assets, &books, &out, cfg).await,
            Err(e) => End::Config(e),
        };
        for (a, _) in &assets {
            invalidate(&ctx, &books, &out, &a.ticker);
        }
        let (level, text, pause) = match (end, ctx.int("venue.ws_reconnect_pause_ms")) {
            (End::Done, _) => return,
            (End::Broken(r), Ok(p)) => (
                Level::Warn,
                format!("WebSocket {name}: {r}; переподключение"),
                Some(p),
            ),
            (End::Broken(_), Err(e)) => (
                Level::Error,
                format!("WebSocket {name} остановлен: {}", e.message),
                None,
            ),
            (End::Config(r), _) => (
                Level::Error,
                format!("WebSocket {name} остановлен: {r}"),
                None,
            ),
        };
        ctx.log(level, "venue.ws", text).await;
        match pause {
            Some(ms) => tokio::time::sleep(Duration::from_millis(ms)).await,
            None => return,
        }
    }
}

/// Объявляет стакан монеты недостоверным: убирает из памяти и сообщает ядру (без ожидания). Ошибок нет.
pub fn invalidate(ctx: &Ctx, books: &Books, out: &mpsc::Sender<MarketEvent>, ticker: &str) {
    books.drop_ticker(ticker);
    let _ = out.try_send(MarketEvent::Invalidated {
        venue: ctx.venue(),
        ticker: ticker.to_string(),
    });
}

/// Читает настройки соединения. Ошибка — текст причины.
pub fn config(ctx: &Ctx) -> Result<Cfg, String> {
    let m = |e: crate::gateway::errors::GatewayError| e.message;
    let stream = match ctx.text("venue.book_stream").map_err(m)?.as_str() {
        "depth" => Stream::Depth,
        "depth_update" => Stream::Update,
        other => {
            return Err(format!(
                "venue.book_stream = {other}: у XT depth или depth_update"
            ));
        }
    };
    let levels = ctx.int("venue.depth_levels").map_err(m)?;
    if stream == Stream::Depth && !DEPTH_LEVELS.contains(&levels) {
        return Err(format!(
            "venue.depth_levels = {levels}: снимок depth XT — только {DEPTH_LEVELS:?}"
        ));
    }
    if levels == 0 {
        return Err("venue.depth_levels = 0: нужен хотя бы один уровень".into());
    }
    if stream == Stream::Update {
        super::snapshots::config(ctx)?;
    }
    let per_sec = ctx.int("venue.ws_subs_per_sec").map_err(m)?.max(1);
    let ping = ctx.int("venue.ws_ping_ms").map_err(m)?;
    if ping == 0 || ping >= SERVER_SILENCE_MS {
        return Err(format!(
            "venue.ws_ping_ms = {ping}: у XT меньше {SERVER_SILENCE_MS} мс (без ping минуту — разрыв)"
        ));
    }
    let silence = ctx.int("venue.ws_silence_ms").map_err(m)?;
    if silence <= ping {
        return Err(format!(
            "venue.ws_silence_ms = {silence} должна быть больше venue.ws_ping_ms = {ping}"
        ));
    }
    Ok(Cfg {
        url: ctx.text("venue.ws_url").map_err(m)?,
        stream,
        levels,
        sub_gap: Duration::from_secs_f64(1.0 / per_sec as f64),
        per_msg: ctx.int("venue.ws_streams_per_msg").map_err(m)?.max(1) as usize,
        ping: Duration::from_millis(ping),
        silence: Duration::from_millis(silence),
        connect_timeout: ctx.http_timeout().map_err(m)?,
        resync_pause: Duration::from_millis(ctx.int("venue.ws_reconnect_pause_ms").map_err(m)?),
    })
}

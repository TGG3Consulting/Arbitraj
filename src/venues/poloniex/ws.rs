//! Файл: venues/poloniex/ws.rs
//! Что делает: жизненный цикл одного соединения WebSocket с рыночными данными Poloniex: настройки, сессия
//! (ws_conn.rs), при обрыве — все стаканы соединения недостоверны, журнал, пауза, переподключение. Нет нужной настройки
//! — соединение останавливается (без бесконечных повторов).
//! Кто вызывает: market.rs (по задаче на соединение; config — проверка до запуска).
//! От чего зависит: ws_conn.rs, book.rs, ctx.rs.
//! Настройки: venue.ws_url, venue.ws_subs_per_sec (≤ 500), venue.ws_ping_ms (< 30 000), venue.ws_silence_ms,
//! venue.ws_reconnect_pause_ms, venue.depth_levels (5, 10 или 20) [poloniex], http.timeout_ms (время на подключение).
//!
//! Официально (Spot → WebSocket, https://api-docs.poloniex.com/spot/websocket/): публичный адрес
//! «wss://ws.poloniex.com/ws/public» (настройка venue.ws_url); «500 requests per second» на соединение; «The server
//! expects a message or a ping every 30 seconds, or it ends the session without notice», сервер сам ping не шлёт.
//! Канал book — «depth»: 5 (по умолчанию), 10 или 20 (Market Data → Book).

use std::time::Duration;

use tokio::sync::mpsc;

use crate::core::types::Asset;
use crate::gateway::events::MarketEvent;
use crate::storage::journal::Level;

use super::book::Books;
use super::ctx::Ctx;
use super::ws_conn;

/// Предел сообщений клиента на соединение: «500 requests per second».
pub const MAX_MSGS_PER_SEC: u64 = 500;
/// Без сообщения или ping дольше — сервер рвёт сессию: «every 30 seconds».
pub const PING_MAX_MS: u64 = 30_000;
/// Уровни снимка стакана канала book: «5, 10, or 20».
pub const DEPTH_LEVELS: [u64; 3] = [5, 10, 20];

/// Почему сессия закончилась.
pub enum End {
    /// Канал ядра закрыт — работа окончена.
    Done,
    /// Обрыв — переподключиться.
    Broken(String),
    /// Нужная настройка не задана — соединение останавливается.
    Config(String),
}

/// Настройки соединения.
pub struct Cfg {
    pub url: String,
    /// Уровней снимка (depth).
    pub depth: u64,
    pub sub_gap: Duration,
    pub ping: Duration,
    pub silence: Duration,
    pub connect_timeout: Duration,
}

/// Работает, пока жив канал out: подключается заново после каждого обрыва.
/// Принимает: окружение, имя соединения, монеты (монета, символ Poloniex), стаканы, канал ядра.
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

/// Объявляет стакан монеты недостоверным: убирает из памяти и сообщает ядру (без ожидания).
pub fn invalidate(ctx: &Ctx, books: &Books, out: &mpsc::Sender<MarketEvent>, ticker: &str) {
    books.drop_ticker(ticker);
    let _ = out.try_send(MarketEvent::Invalidated {
        venue: ctx.venue(),
        ticker: ticker.to_string(),
    });
}

/// Пинг и тишина: ping меньше 30 с, тишина больше ping. Ошибка — текст причины. Общая для рынка и потока аккаунта.
pub fn ping_silence(ctx: &Ctx) -> Result<(Duration, Duration), String> {
    let m = |e: crate::gateway::errors::GatewayError| e.message;
    let ping = ctx.int("venue.ws_ping_ms").map_err(m)?;
    if ping >= PING_MAX_MS {
        return Err(format!(
            "venue.ws_ping_ms = {ping}: Poloniex рвёт сессию без сообщения 30 с — нужно меньше {PING_MAX_MS}"
        ));
    }
    let silence = ctx.int("venue.ws_silence_ms").map_err(m)?;
    if silence <= ping {
        return Err(format!(
            "venue.ws_silence_ms = {silence} должна быть больше venue.ws_ping_ms = {ping} (pong приходит на ping)"
        ));
    }
    Ok((Duration::from_millis(ping), Duration::from_millis(silence)))
}

/// Читает настройки соединения. Ошибка — текст «не задана …» или «вне пределов документации».
pub fn config(ctx: &Ctx) -> Result<Cfg, String> {
    let m = |e: crate::gateway::errors::GatewayError| e.message;
    let per_sec = ctx.int("venue.ws_subs_per_sec").map_err(m)?;
    if per_sec == 0 || per_sec > MAX_MSGS_PER_SEC {
        return Err(format!(
            "venue.ws_subs_per_sec = {per_sec}: у Poloniex от 1 до {MAX_MSGS_PER_SEC} сообщений в секунду"
        ));
    }
    let depth = ctx.int("venue.depth_levels").map_err(m)?;
    if !DEPTH_LEVELS.contains(&depth) {
        return Err(format!(
            "venue.depth_levels = {depth}: у канала book Poloniex только {DEPTH_LEVELS:?}"
        ));
    }
    let (ping, silence) = ping_silence(ctx)?;
    Ok(Cfg {
        url: ctx.text("venue.ws_url").map_err(m)?,
        depth,
        sub_gap: Duration::from_secs_f64(1.0 / per_sec as f64),
        ping,
        silence,
        connect_timeout: ctx.http_timeout().map_err(m)?,
    })
}

//! Файл: venues/coinex/ws.rs
//! Что делает: жизненный цикл одного соединения WebSocket с рыночными данными CoinEx: настройки, сессия (ws_conn.rs),
//! при обрыве — все стаканы соединения недостоверны, журнал, пауза, переподключение. Нет нужной настройки — соединение
//! останавливается (без бесконечных повторов).
//! Кто вызывает: market.rs (по задаче на соединение; config — проверка до запуска).
//! От чего зависит: ws_conn.rs, book.rs, ctx.rs.
//! Настройки: venue.ws_url, venue.ws_ping_ms, venue.ws_silence_ms, venue.ws_reconnect_pause_ms, venue.depth_levels
//! (5, 10, 20 или 50) [coinex], http.timeout_ms (время на подключение), http.max_body_bytes (предел распакованного кадра).
//!
//! Официально:
//! - Integration Guide (https://docs.coinex.com/api/v2/guide): спот «wss://socket.coinex.com/v2/spot» (настройка
//!   venue.ws_url); ответы сжаты (inflate.rs).
//! - WS Market Depth (https://docs.coinex.com/api/v2/spot/market/ws/market-depth): limit — 5, 10, 20, 50; interval "0"
//!   (без склейки); if_full = true — каждый пуш полный снимок N уровней (без инкрементов и пересинхронизаций); пуш
//!   «about every 200 ms when depth has changed. No push occurs without an update», полный — «every 1 minute».
//! - Ping (https://docs.coinex.com/api/v2/common/ws/ping): server.ping → "pong"; интервал и таймаут простоя в
//!   документации НЕ указаны → venue.ws_ping_ms задаёт Тигран по живому подключению.

use std::time::Duration;

use tokio::sync::mpsc;

use crate::core::types::Asset;
use crate::gateway::events::MarketEvent;
use crate::settings::read::At;
use crate::storage::journal::Level;

use super::book::Books;
use super::ctx::Ctx;
use super::ws_conn;

/// Уровни стакана — значения limit официальной документации depth.subscribe.
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

/// Настройки соединения.
pub struct Cfg {
    pub url: String,
    /// limit depth.subscribe.
    pub levels: u64,
    pub ping: Duration,
    pub silence: Duration,
    pub connect_timeout: Duration,
    /// Предел распакованного кадра, байт.
    pub max_frame: usize,
}

/// Работает, пока жив канал out: подключается заново после каждого обрыва.
/// Принимает: окружение, имя соединения, монеты (монета, рынок CoinEx), стаканы, канал ядра.
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

/// Читает настройки соединения. Ошибка — текст «не задана …» или «вне пределов документации».
pub fn config(ctx: &Ctx) -> Result<Cfg, String> {
    let m = |e: crate::gateway::errors::GatewayError| e.message;
    let levels = ctx.int("venue.depth_levels").map_err(m)?;
    if !DEPTH_LEVELS.contains(&levels) {
        return Err(format!(
            "venue.depth_levels = {levels}: у CoinEx limit стакана только {DEPTH_LEVELS:?}"
        ));
    }
    let ping = ctx.int("venue.ws_ping_ms").map_err(m)?;
    let silence = ctx.int("venue.ws_silence_ms").map_err(m)?;
    if silence <= ping {
        return Err(format!(
            "venue.ws_silence_ms = {silence} должна быть больше venue.ws_ping_ms = {ping} (ответ на server.ping — тоже кадр)"
        ));
    }
    let max_frame = ctx
        .settings
        .integer("http.max_body_bytes", At::default())
        .map_err(m)? as usize;
    Ok(Cfg {
        url: ctx.text("venue.ws_url").map_err(m)?,
        levels,
        ping: Duration::from_millis(ping),
        silence: Duration::from_millis(silence),
        connect_timeout: ctx.http_timeout().map_err(m)?,
        max_frame,
    })
}

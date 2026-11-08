//! Файл: venues/phemex/ws.rs
//! Что делает: жизненный цикл одного соединения WebSocket с рыночными данными Phemex: настройки, сессия (ws_conn.rs),
//! при обрыве — все стаканы соединения недостоверны, журнал, пауза, переподключение. Нет нужной настройки — соединение
//! останавливается (без бесконечных повторов).
//! Кто вызывает: market.rs (по задаче на соединение; config — проверка до запуска), user_stream_conn.rs (config_ping).
//! От чего зависит: ws_conn.rs, book.rs, symbols.rs, ctx.rs, gateway/feed_reg.rs (решение 202).
//! Настройки: venue.ws_url, venue.ws_subs_per_sec (≤ 20), venue.ws_ping_ms (< 30 000), venue.ws_silence_ms,
//! venue.ws_reconnect_pause_ms, venue.depth_levels (1…30), venue.book_seq_strict [phemex], http.timeout_ms.
//!
//! Официально (Spot Websocket API, https://phemex-docs.github.io/#heartbeat-2 и #subscribe-orderbook-2): адрес
//! «wss://ws.phemex.com (further URI or querystring not allowed)» (настройка venue.ws_url); клиент шлёт ping «with
//! interval less than 30 seconds, otherwise DataGW will drop the connection», рекомендовано 5 с и переподключение, если
//! «don't receive messages in 3 heartbeat intervals»; orderbook.subscribe — «depth = 30 and interval = 20ms»;
//! «Each connection has throttle limit to 20 request/s» (API Rate Limits).

use std::time::Duration;

use tokio::sync::mpsc;

use crate::core::types::Asset;
use crate::gateway::events::MarketEvent;
use crate::storage::journal::Level;

use super::book::Books;
use super::ctx::Ctx;
use super::symbols::Symbols;
use super::ws_conn;

/// Запросов клиента на соединение в секунду — «throttle limit to 20 request/s».
pub const MAX_MSGS_PER_SEC: u64 = 20;
/// Ping не реже — «interval less than 30 seconds».
pub const PING_MAX_MS: u64 = 30_000;
/// Уровней в канале orderbook.subscribe — «depth = 30».
pub const DEPTH_MAX: u64 = 30;

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
    pub sub_gap: Duration,
    pub ping: Duration,
    pub silence: Duration,
    pub connect_timeout: Duration,
    /// Уровней на сторону, которые отдаются ядру.
    pub levels: usize,
    /// venue.book_seq_strict: номер sequence изменения должен быть ровно прежний + 1.
    pub strict: bool,
}

/// Работает, пока жив канал out: подключается заново после каждого обрыва.
/// Принимает: окружение, имя соединения, монеты (монета, символ Phemex), справочник (масштабы), стаканы, канал ядра.
pub async fn run(
    ctx: Ctx,
    name: String,
    assets: Vec<(Asset, String)>,
    symbols: Symbols,
    books: Books,
    out: mpsc::Sender<MarketEvent>,
) {
    // Решение 202: свежесть пар соединения и команды сторожа ядра (переподписать пару, переподключить соединение).
    let tickers: Vec<String> = assets.iter().map(|(a, _)| a.ticker.clone()).collect();
    let (feed, mut cmd) = books.feed.register(&name, &tickers);
    while !out.is_closed() {
        let end = match config(&ctx) {
            Ok(cfg) => {
                let w = ws_conn::Watch {
                    feed: &feed,
                    cmd: &mut cmd,
                };
                ws_conn::session(&ctx, &name, &assets, &symbols, &books, &out, cfg, w).await
            }
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
        address: None,
    });
}

/// Ping и тишина (общие для рынка и потока аккаунта). Ошибка — текст «не задана …» или «вне пределов документации».
pub fn config_ping(ctx: &Ctx) -> Result<(Duration, Duration), String> {
    let m = |e: crate::gateway::errors::GatewayError| e.message;
    let ping = ctx.int("venue.ws_ping_ms").map_err(m)?;
    if ping == 0 || ping >= PING_MAX_MS {
        return Err(format!(
            "venue.ws_ping_ms = {ping}: Phemex рвёт соединение без ping 30 с — нужно от 1 до {} (совет документации — 5000)",
            PING_MAX_MS - 1
        ));
    }
    let silence = ctx.int("venue.ws_silence_ms").map_err(m)?;
    if silence <= ping {
        return Err(format!(
            "venue.ws_silence_ms = {silence} должна быть больше venue.ws_ping_ms = {ping} (на ping приходит pong)"
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
            "venue.ws_subs_per_sec = {per_sec}: у Phemex от 1 до {MAX_MSGS_PER_SEC} запросов в секунду на соединение"
        ));
    }
    let levels = ctx.int("venue.depth_levels").map_err(m)?;
    if levels == 0 || levels > DEPTH_MAX {
        return Err(format!(
            "venue.depth_levels = {levels}: канал orderbook.subscribe Phemex даёт до {DEPTH_MAX} уровней"
        ));
    }
    let (ping, silence) = config_ping(ctx)?;
    Ok(Cfg {
        url: ctx.text("venue.ws_url").map_err(m)?,
        sub_gap: Duration::from_secs_f64(1.0 / per_sec as f64),
        ping,
        silence,
        connect_timeout: ctx.http_timeout().map_err(m)?,
        levels: levels as usize,
        strict: ctx.flag("venue.book_seq_strict").unwrap_or(false),
    })
}

//! Файл: venues/binance/ws.rs
//! Что делает: жизненный цикл одного соединения WebSocket рынка Binance: чтение настроек (с пределами документации),
//! сессия (ws_conn.rs), при обрыве — все стаканы соединения недостоверны, журнал, пауза, переподключение. Нет нужной
//! настройки — соединение останавливается (без бесконечных повторов).
//! Кто вызывает: market.rs (по задаче на соединение), feed.rs (config — окна).
//! От чего зависит: ws_conn.rs, book.rs, ctx.rs.
//! Настройки: venue.ws_url, venue.depth_levels, venue.book_interval, venue.ws_subs_per_sec, venue.ws_streams_per_msg,
//! venue.ws_silence_ms, venue.ws_reconnect_pause_ms [binance], http.timeout_ms.
//!
//! Официально (web-socket-streams «General WSS information», «WebSocket Limits»): адрес wss://stream.binance.com:9443
//! (или :443), комбинированный поток — путь /stream; соединение живёт 24 ч; сервер шлёт ping раз в 20 с, pong — не
//! позже минуты; «5 incoming messages per second» — включая PING, PONG и SUBSCRIBE (превышение — разрыв, повторные —
//! бан IP): подписок в секунду строго меньше 5 (место для pong); до 1024 потоков на соединение; уровни 5, 10, 20;
//! интервал 1000ms или 100ms. Сколько потоков в одном SUBSCRIBE — в документации не сказано (venue.ws_streams_per_msg).

use std::time::Duration;

use tokio::sync::mpsc;

use crate::core::types::Asset;
use crate::gateway::events::MarketEvent;
use crate::storage::journal::Level;

use super::book::Books;
use super::ctx::Ctx;
use super::ws_conn;

/// Предел входящих сообщений на соединение в секунду — официально 5 (включая pong): подписок — строго меньше.
pub const MAX_MSGS_PER_SEC: u64 = 5;
/// Допустимые уровни частичного стакана — официально 5, 10, 20.
pub const DEPTH_LEVELS: [u64; 3] = [5, 10, 20];
/// Допустимые интервалы — официально 1000ms или 100ms.
pub const INTERVALS: [&str; 2] = ["100ms", "1000ms"];

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
    /// Адрес комбинированного потока: venue.ws_url + /stream.
    pub url: String,
    pub levels: u64,
    /// Окончание имени потока: «@100ms» или пусто (1000ms — имя без окончания).
    pub suffix: &'static str,
    pub sub_gap: Duration,
    pub per_msg: usize,
    pub silence: Duration,
    pub connect_timeout: Duration,
}

/// Работает, пока жив канал out: подключается заново после каждого обрыва.
/// Принимает: окружение, имя соединения, монеты (монета, символ Binance), стаканы, канал ядра.
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
        address: None,
    });
}

/// Читает настройки соединения (и соединений окна, feed.rs). Ошибка — текст причины.
pub fn config(ctx: &Ctx) -> Result<Cfg, String> {
    let m = |e: crate::gateway::errors::GatewayError| e.message;
    let per_sec = ctx.int("venue.ws_subs_per_sec").map_err(m)?;
    if per_sec == 0 || per_sec >= MAX_MSGS_PER_SEC {
        return Err(format!(
            "venue.ws_subs_per_sec = {per_sec}: у Binance всего {MAX_MSGS_PER_SEC} сообщений/с на соединение, включая pong — подписок от 1 до {}",
            MAX_MSGS_PER_SEC - 1
        ));
    }
    let levels = ctx.int("venue.depth_levels").map_err(m)?;
    if !DEPTH_LEVELS.contains(&levels) {
        return Err(format!(
            "venue.depth_levels = {levels}: Binance — только {DEPTH_LEVELS:?}"
        ));
    }
    let interval = ctx.text("venue.book_interval").map_err(m)?;
    let suffix = match interval.as_str() {
        "100ms" => "@100ms",
        "1000ms" => "",
        other => {
            return Err(format!(
                "venue.book_interval = {other}: Binance — только {INTERVALS:?}"
            ));
        }
    };
    let per_msg = ctx.int("venue.ws_streams_per_msg").map_err(m)?.max(1) as usize;
    Ok(Cfg {
        url: format!(
            "{}/stream",
            ctx.text("venue.ws_url").map_err(m)?.trim_end_matches('/')
        ),
        levels,
        suffix,
        sub_gap: Duration::from_secs_f64(1.0 / per_sec as f64),
        per_msg,
        silence: Duration::from_millis(ctx.int("venue.ws_silence_ms").map_err(m)?),
        connect_timeout: ctx.http_timeout().map_err(m)?,
    })
}

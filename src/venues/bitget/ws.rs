//! Файл: venues/bitget/ws.rs
//! Что делает: жизненный цикл одного соединения WebSocket с рыночными данными Bitget: настройки, сессия (ws_conn.rs),
//! при обрыве — все стаканы соединения недостоверны, журнал, пауза, переподключение. Нет нужной настройки — соединение
//! останавливается (без бесконечных повторов).
//! Кто вызывает: market.rs (по задаче на соединение; config — проверка до запуска).
//! От чего зависит: ws_conn.rs, book.rs, ctx.rs, gateway/feed_reg.rs (решение 202).
//! Настройки: venue.ws_url, venue.ws_subs_per_sec (× 5 ≤ 50), venue.ws_ping_ms (< 120 000), venue.ws_silence_ms,
//! venue.ws_reconnect_pause_ms, venue.depth_levels (1, 5 или 15) [bitget], http.timeout_ms (время на подключение).
//!
//! Официально (WebSocket intro, https://www.bitget.com/docs/classic/websocket/intro): публичный адрес
//! «wss://ws.bitget.com/v2/ws/public» (настройка venue.ws_url); сообщения клиента «(ping, login, subscribe,
//! unsubscribe) are rate limited», «Limits are calculated per WebSocket connection, and separately for each message
//! type» — за 5 с: ping 10 (превышение — соединение закрывается, код 30007), login 2, subscribe 50, unsubscribe 50
//! (лишние отбрасываются); прежняя фраза «up to 10 messages per second» на странице сверки 08.11 уже не стоит (по числу
//! для подписок предел тот же: 50 за 5 с = 10 в секунду). Переподписка пары сторожем (решение 202) тратит 1 subscribe и
//! 1 unsubscribe из тех же 50 за 5 с; клиент шлёт "ping", «Websocket server will disconnect the connection if there is no
//! string "ping" received for 2 min»; каналы стакана снимками — books1, books5, books15 (Depth Channel).

use std::time::Duration;

use tokio::sync::mpsc;

use crate::core::types::Asset;
use crate::gateway::events::MarketEvent;
use crate::storage::journal::Level;

use super::book::Books;
use super::ctx::Ctx;
use super::ws_conn;

/// Предел сообщений subscribe на соединение за окно MSG_WINDOW_S — WebSocket intro: «subscribe» — 50 сообщений за 5 с.
pub const SUB_MSGS_PER_WINDOW: u64 = 50;
/// Окно лимитов сообщений клиента, с — WebSocket intro: лимиты «over 5-second intervals».
pub const MSG_WINDOW_S: u64 = 5;
/// Без "ping" дольше — сервер рвёт соединение: «no string "ping" received for 2 min».
pub const PING_MAX_MS: u64 = 120_000;
/// Уровни снимка стакана — каналы books1, books5, books15 официальной документации.
pub const DEPTH_LEVELS: [u64; 3] = [1, 5, 15];

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
    /// Канал стакана: books1 / books5 / books15.
    pub channel: String,
    pub sub_gap: Duration,
    pub ping: Duration,
    pub silence: Duration,
    pub connect_timeout: Duration,
}

/// Работает, пока жив канал out: подключается заново после каждого обрыва.
/// Принимает: окружение, имя соединения, монеты (монета, символ Bitget), стаканы, канал ядра.
pub async fn run(
    ctx: Ctx,
    name: String,
    assets: Vec<(Asset, String)>,
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
                ws_conn::session(&ctx, &name, &assets, &books, &out, cfg, w).await
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

/// Читает настройки соединения. Ошибка — текст «не задана …» или «вне пределов документации».
pub fn config(ctx: &Ctx) -> Result<Cfg, String> {
    let m = |e: crate::gateway::errors::GatewayError| e.message;
    let per_sec = ctx.int("venue.ws_subs_per_sec").map_err(m)?;
    if per_sec == 0 || per_sec * MSG_WINDOW_S > SUB_MSGS_PER_WINDOW {
        return Err(format!(
            "venue.ws_subs_per_sec = {per_sec}: у Bitget не больше {SUB_MSGS_PER_WINDOW} подписок за {MSG_WINDOW_S} с на соединение — от 1 до {} в секунду",
            SUB_MSGS_PER_WINDOW / MSG_WINDOW_S
        ));
    }
    let levels = ctx.int("venue.depth_levels").map_err(m)?;
    if !DEPTH_LEVELS.contains(&levels) {
        return Err(format!(
            "venue.depth_levels = {levels}: у Bitget снимками только {DEPTH_LEVELS:?} (books1, books5, books15)"
        ));
    }
    let ping = ctx.int("venue.ws_ping_ms").map_err(m)?;
    if ping >= PING_MAX_MS {
        return Err(format!(
            "venue.ws_ping_ms = {ping}: Bitget рвёт соединение без ping 2 минуты — нужно меньше {PING_MAX_MS}"
        ));
    }
    let silence = ctx.int("venue.ws_silence_ms").map_err(m)?;
    if silence <= ping {
        return Err(format!(
            "venue.ws_silence_ms = {silence} должна быть больше venue.ws_ping_ms = {ping} (pong приходит на ping)"
        ));
    }
    Ok(Cfg {
        url: ctx.text("venue.ws_url").map_err(m)?,
        channel: format!("books{levels}"),
        sub_gap: Duration::from_secs_f64(1.0 / per_sec as f64),
        ping: Duration::from_millis(ping),
        silence: Duration::from_millis(silence),
        connect_timeout: ctx.http_timeout().map_err(m)?,
    })
}

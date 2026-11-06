//! Файл: venues/gate/ws.rs
//! Что делает: жизненный цикл одного соединения WebSocket с рыночными данными Gate:
//! чтение настроек, сессия, при обрыве — все стаканы соединения недостоверны, журнал, пауза,
//! переподключение. Нет нужной настройки — соединение останавливается (без бесконечных повторов).
//! Кто вызывает: market.rs (по задаче на соединение).
//! От чего зависит: ws_conn.rs (сама сессия), book.rs, ctx.rs, gateway/feed_health.rs (решение 202).
//! Настройки: venue.ws_url, venue.ws_subs_per_sec, venue.ws_silence_ms, venue.ws_reconnect_pause_ms,
//! venue.ws_ping_ms, venue.depth_levels, venue.book_interval, http.timeout_ms (время на подключение).
//!
//! Официально (Spot WebSocket v4, https://www.gate.com/docs/developers/apiv4/ws/en/): адрес «wss://api.gateio.ws/ws/v4/»
//! (задаётся настройкой venue.ws_url); данные — JSON; пинг приложения spot.ping → spot.pong сбрасывает таймер
//! простоя сервера (срок простоя в документации не назван — venue.ws_ping_ms задаёт Тигран, в официальном примере —
//! раз в 10 с); сервер шлёт и пинг протокола — отвечаем Pong. Предела подписок на соединение и срока жизни соединения
//! в документации нет (вопрос Тиграну, docs/TASKS.md).

use std::time::Duration;

use tokio::sync::mpsc;

use crate::core::types::Asset;
use crate::gateway::events::MarketEvent;
use crate::storage::journal::Level;

use super::book::Books;
use super::ctx::{Ctx, ID};
use super::ws_conn;

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
    pub levels: u64,
    /// Интервал снимков spot.order_book (venue.book_interval [gate]).
    pub interval: String,
    pub sub_gap: Duration,
    pub silence: Duration,
    /// Как часто слать spot.ping (venue.ws_ping_ms [gate]).
    pub ping: Duration,
    pub connect_timeout: Duration,
}

/// Работает, пока жив канал out: подключается заново после каждого обрыва.
/// Принимает: окружение, имя соединения, монеты (монета, символ Gate), стаканы, канал ядра.
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
        if matches!(end, End::Done) {
            return;
        }
        for (a, _) in &assets {
            invalidate(&ctx, &books, &out, &a.ticker);
        }
        let (level, text, pause) = match (end, ctx.int("venue.ws_reconnect_pause_ms")) {
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
            (End::Done, _) => return,
        };
        let _ = ctx
            .journal
            .write(level, "venue.ws", Some(ID), text, None)
            .await;
        match pause {
            Some(ms) => tokio::time::sleep(Duration::from_millis(ms)).await,
            None => return,
        }
    }
}

/// Объявляет стакан монеты недостоверным: убирает из памяти и сообщает ядру (без ожидания).
pub fn invalidate(ctx: &Ctx, books: &Books, out: &mpsc::Sender<MarketEvent>, ticker: &str) {
    books.drop_ticker(ticker);
    // Канал ядра переполнен — стакан уже убран из памяти, котировки по нему не будет.
    let _ = out.try_send(MarketEvent::Invalidated {
        venue: ctx.venue(),
        ticker: ticker.to_string(),
    });
}

/// Читает настройки соединения (и соединений окна, feed.rs). Ошибка — текст «не задана …».
pub fn config(ctx: &Ctx) -> Result<Cfg, String> {
    let per_sec = ctx.int("venue.ws_subs_per_sec").map_err(|e| e.message)?;
    Ok(Cfg {
        url: ctx.text("venue.ws_url").map_err(|e| e.message)?,
        levels: ctx.int("venue.depth_levels").map_err(|e| e.message)?,
        interval: ctx.text("venue.book_interval").map_err(|e| e.message)?,
        // Не меньше 1 мс между подписками: при очень большом значении настройки интервал не обнуляется.
        sub_gap: std::time::Duration::from_secs_f64(1.0 / per_sec.max(1) as f64)
            .max(Duration::from_millis(1)),
        silence: Duration::from_millis(ctx.int("venue.ws_silence_ms").map_err(|e| e.message)?),
        ping: Duration::from_millis(ctx.int("venue.ws_ping_ms").map_err(|e| e.message)?),
        connect_timeout: ctx.http_timeout().map_err(|e| e.message)?,
    })
}

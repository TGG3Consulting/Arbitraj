//! Файл: venues/kucoin/ws.rs
//! Что делает: жизненный цикл одного соединения WebSocket с рыночными данными KuCoin: настройки, новый публичный токен
//! (bullet.rs) на каждое подключение, сессия (ws_conn.rs), при обрыве — все стаканы соединения недостоверны, журнал,
//! пауза, переподключение. Нет нужной настройки — соединение останавливается (без бесконечных повторов).
//! Кто вызывает: market.rs (по задаче на соединение), feed.rs (config).
//! От чего зависит: ws_conn.rs, bullet.rs, book.rs, ctx.rs.
//! Настройки: venue.ws_subs_per_sec (≤ 10), venue.ws_reconnect_pause_ms, venue.depth_levels (5 или 50) [kucoin],
//! http.timeout_ms (время на подключение). Адрес, пинг и срок понга — из ответа bullet (не настройки).
//!
//! Официально (https://www.kucoin.com/docs-new/rate-limit-rule-classic): «100 per 10 seconds» сообщений клиента на
//! соединение, «Maximum 100» тем в одном запросе подписки, «Spot/Margin is limited to 400» тем на соединение; сервер
//! может разорвать соединение при переключении балансировки («at most once or twice per week») — переподключаться.

use std::time::Duration;

use tokio::sync::mpsc;

use crate::core::types::Asset;
use crate::gateway::events::MarketEvent;
use crate::storage::journal::Level;

use super::book::Books;
use super::ctx::Ctx;
use super::ws_conn;

/// Предел сообщений клиента: «100 per 10 seconds» — не больше 10 в секунду.
pub const MAX_MSGS_PER_SEC: u64 = 10;
/// Допустимые уровни снимка стакана — темы level2Depth5 и level2Depth50 официальной документации.
pub const DEPTH_LEVELS: [u64; 2] = [5, 50];

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
    pub levels: u64,
    pub sub_gap: Duration,
    pub connect_timeout: Duration,
}

/// Работает, пока жив канал out: подключается заново после каждого обрыва.
/// Принимает: окружение, имя соединения, монеты (монета, символ KuCoin), стаканы, канал ядра.
pub async fn run(
    ctx: Ctx,
    name: String,
    assets: Vec<(Asset, String)>,
    books: Books,
    out: mpsc::Sender<MarketEvent>,
) {
    while !out.is_closed() {
        let end = match config(&ctx) {
            Ok(cfg) => match super::bullet::get(&ctx, false).await {
                Ok(t) => ws_conn::session(&ctx, &name, &assets, &books, &out, cfg, t).await,
                Err(e) => End::Broken(format!("токен bullet-public не получен: {}", e.message)),
            },
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
    let per_sec = ctx.int("venue.ws_subs_per_sec").map_err(|e| e.message)?;
    if per_sec == 0 || per_sec > MAX_MSGS_PER_SEC {
        return Err(format!(
            "venue.ws_subs_per_sec = {per_sec}: у KuCoin не больше {MAX_MSGS_PER_SEC} сообщений в секунду (100 за 10 с)"
        ));
    }
    let levels = ctx.int("venue.depth_levels").map_err(|e| e.message)?;
    if !DEPTH_LEVELS.contains(&levels) {
        return Err(format!(
            "venue.depth_levels = {levels}: у KuCoin только {DEPTH_LEVELS:?}"
        ));
    }
    Ok(Cfg {
        levels,
        sub_gap: Duration::from_secs_f64(1.0 / per_sec as f64),
        connect_timeout: ctx.http_timeout().map_err(|e| e.message)?,
    })
}

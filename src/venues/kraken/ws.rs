//! Файл: venues/kraken/ws.rs
//! Что делает: жизненный цикл одного соединения WebSocket стаканов Kraken: настройки, сессия (ws_conn.rs), при обрыве —
//! все стаканы соединения недостоверны, журнал, пауза, переподключение (через ограничитель попыток ws_req.rs). Нет
//! нужной настройки — соединение останавливается (без бесконечных повторов).
//! Кто вызывает: market.rs (по задаче на соединение), user_stream_conn.rs (config_ping — те же сроки пинга).
//! От чего зависит: ws_conn.rs, book.rs, ctx.rs, market.rs (Sub), gateway/feed_reg.rs (решение 202).
//! Настройки: venue.ws_url, venue.depth_levels (10, 25, 100, 500, 1000), venue.ws_ping_ms, venue.ws_silence_ms,
//! venue.ws_streams_per_msg, venue.ws_subs_per_sec, venue.ws_reconnect_pause_ms [kraken], http.timeout_ms;
//! решение 426 — venue.book_standby_depth [kraken] (глубина соединения-страховки; не задана — страховки нет).
//!
//! Решение 426: соединение — основное или страховка (standby_emit.rs Link). Страховка подписывает те же пары с глубиной
//! venue.book_standby_depth, учёт её свежести — вне сторожа ядра (standby.rs Arbiter::feed); обрыв любого соединения —
//! его пары теряют стакан через выбор (у основного — сразу стакан страховки, где он сверен), а не «недостоверен» сразу.
//! Настройка страховки снята — соединение-страховка останавливается (журнал); возвращается со следующей подпиской.
//!
//! Официально: адрес публичного WebSocket v2 — wss://ws.kraken.com/v2 (https://docs.kraken.com/exchange/api-reference/
//! spot-websocket.md; только настройка); глубина book — «10, 25, 100, 500, 1000» (spot-websocket-v2/book.md); сервер
//! закрывает соединение «within approximately one minute of inactivity», ping — «at least every 60 seconds».

use std::time::Duration;

use tokio::sync::mpsc;

use crate::gateway::core_out_book::{BookOut, Fed};
use crate::gateway::errors::ErrorCode;
use crate::gateway::events::MarketEvent;
use crate::storage::journal::Level;

use super::book::Books;
use super::ctx::Ctx;
use super::market::Sub;
use super::standby::Role;
use super::standby_emit::{Link, Out};
use super::ws_conn;

/// Глубины канала book — официальный перечень depth.
pub const DEPTHS: [u64; 5] = [10, 25, 100, 500, 1000];
/// Срок тишины, после которого Kraken закрывает соединение, мс — «approximately one minute»: пинг должен быть чаще.
pub const SERVER_SILENCE_MS: u64 = 60_000;

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
    /// Глубина подписки (уровней на сторону).
    pub depth: u64,
    pub ping: Duration,
    pub silence: Duration,
    pub connect_timeout: Duration,
    /// Символов в одном запросе подписки.
    pub per_msg: usize,
    /// Пауза между запросами подписки (1 / venue.ws_subs_per_sec).
    pub sub_gap: Duration,
    /// Повторный разрыв той же пары раньше этого — переподключение, а не пересподписка (урок 12).
    pub resync_gap: Duration,
}

/// Работает, пока жив канал out: подключается заново после каждого обрыва.
/// Принимает: окружение, имя соединения, пары, стаканы, канал ядра, роль и выбор стакана (решение 426).
pub async fn run(
    ctx: Ctx,
    name: String,
    subs: Vec<Sub>,
    books: Books,
    out: mpsc::Sender<MarketEvent>,
    link: Link,
) {
    // Решение 202: свежесть пар соединения и команды сторожа ядра (переподписать пару, переподключить соединение).
    // Решение 426: страховка — в своём учёте выбора, сторож ядра её не видит.
    let tickers: Vec<String> = subs.iter().map(|s| s.asset.ticker.clone()).collect();
    let reg = match (&link.arb, link.role) {
        (Some(arb), Role::Standby) => &arb.feed,
        _ => &books.feed,
    };
    let (feed, mut cmd) = reg.register(&name, &tickers);
    let kept = BookOut::new(&out, ctx.venue(), &ctx.settings, &ctx.journal);
    let o = Out {
        ctx: &ctx,
        books: &books,
        out: &out,
        kept: &kept,
    };
    while !out.is_closed() {
        let end = match config_for(&ctx, link.role) {
            Ok(cfg) => {
                let w = ws_conn::Watch {
                    feed: &feed,
                    cmd: &mut cmd,
                    link: &link,
                };
                ws_conn::session(&ctx, &name, &subs, &books, &out, cfg, w).await
            }
            Err(e) => {
                let m = format!("WebSocket {name} остановлен: {e}");
                ctx.log(Level::Error, "venue.ws", m).await;
                // Решение 426: остановленное соединение не держит стаканы своих пар в выборе.
                for s in &subs {
                    o.lose(&link, &s.asset.ticker).await;
                }
                return;
            }
        };
        let End::Broken(why) = end else { return };
        for s in &subs {
            if let Some(Fed::Closed) = o.lose(&link, &s.asset.ticker).await {
                return;
            }
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
        tokio::time::sleep(Duration::from_millis(pause)).await;
    }
}

/// Объявляет стакан монеты недостоверным: убирает из памяти и сообщает ядру (без ожидания).
pub fn invalidate(ctx: &Ctx, books: &Books, out: &mpsc::Sender<MarketEvent>, ticker: &str) {
    books.drop_ticker(ticker);
    // Решение 324: не теряется при полном канале ядра (в общий набор срочным, старый кадр пары снимается).
    crate::gateway::core_out_book::invalidated(out, ctx.venue(), ticker);
}

/// Сроки пинга и тишины: (ping, silence). venue.ws_ping_ms < 60000 (сервер закрывает молчащее ~через минуту).
/// Ошибка — текст «не задана …» или «вне пределов документации».
pub fn config_ping(ctx: &Ctx) -> Result<(Duration, Duration), String> {
    let ping = ctx.int("venue.ws_ping_ms").map_err(|e| e.message)?;
    if ping == 0 || ping >= SERVER_SILENCE_MS {
        return Err(format!(
            "venue.ws_ping_ms = {ping}: у Kraken меньше {SERVER_SILENCE_MS} мс («at least every 60 seconds»)"
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

/// Решение 426: глубина соединения-страховки venue.book_standby_depth. Возвращает: None — не задана (страховки нет).
/// Ошибка — текст: не из перечня канала book («10, 25, 100, 500, 1000») или больше venue.depth_levels (ядру всё равно
/// уходит не больше venue.depth_levels уровней), настройка хранится неверно.
pub fn standby_depth(ctx: &Ctx) -> Result<Option<u64>, String> {
    let d = match ctx.int("venue.book_standby_depth") {
        Ok(d) => d,
        Err(e) if e.code == ErrorCode::NotConfigured => return Ok(None),
        Err(e) => return Err(e.message),
    };
    let main = ctx.int("venue.depth_levels").map_err(|e| e.message)?;
    if !DEPTHS.contains(&d) || d > main {
        return Err(format!(
            "venue.book_standby_depth = {d}: у Kraken 10, 25, 100, 500 или 1000 и не больше venue.depth_levels = {main}"
        ));
    }
    Ok(Some(d))
}

/// Настройки соединения по роли: страховка — та же, что у основного, с глубиной venue.book_standby_depth.
/// Ошибка — как у config; у страховки ещё «venue.book_standby_depth не задана» (страховка снята).
fn config_for(ctx: &Ctx, role: Role) -> Result<Cfg, String> {
    let mut cfg = config(ctx)?;
    if role == Role::Standby {
        cfg.depth = standby_depth(ctx)?
            .ok_or("venue.book_standby_depth не задана — страховка стакана снята")?;
    }
    Ok(cfg)
}

/// Читает настройки соединения. Ошибка — текст «не задана …» или «вне пределов документации».
pub fn config(ctx: &Ctx) -> Result<Cfg, String> {
    let depth = ctx.int("venue.depth_levels").map_err(|e| e.message)?;
    if !DEPTHS.contains(&depth) {
        return Err(format!(
            "venue.depth_levels = {depth}: у Kraken только 10, 25, 100, 500 или 1000 (канал book)"
        ));
    }
    let (ping, silence) = config_ping(ctx)?;
    let pause = ctx
        .int("venue.ws_reconnect_pause_ms")
        .map_err(|e| e.message)?;
    let per_msg = ctx.int("venue.ws_streams_per_msg").map_err(|e| e.message)?;
    let per_sec = ctx.int("venue.ws_subs_per_sec").map_err(|e| e.message)?;
    if per_msg == 0 || per_sec == 0 {
        return Err("venue.ws_streams_per_msg и venue.ws_subs_per_sec должны быть больше 0".into());
    }
    Ok(Cfg {
        url: ctx.text("venue.ws_url").map_err(|e| e.message)?,
        depth,
        ping,
        silence,
        connect_timeout: ctx.http_timeout().map_err(|e| e.message)?,
        per_msg: per_msg as usize,
        sub_gap: Duration::from_secs_f64(1.0 / per_sec as f64),
        resync_gap: Duration::from_millis(pause),
    })
}

//! Файл: venues/htx/fast_ws.rs
//! Что делает: решение 422 — жизненный цикл одного соединения быстрого канала стакана HTX (адрес /feed,
//! market.$symbol.mbp.$levels): настройки, сессия (fast_conn.rs), при обрыве — пары соединения сразу уходят на снимки
//! mbp.refresh (book_mode.rs fall_back: запасной снимок ядру; его нет — стакан пары недостоверен), журнал, пауза,
//! переподключение. Нужная настройка не задана — соединение останавливается, пары остаются на снимках (как до 422).
//! Кто вызывает: market.rs (по задаче на соединение; config — до запуска).
//! От чего зависит: fast_conn.rs, snap_out.rs (back), book.rs, book_mode.rs, ws.rs (End), ctx.rs.
//! Настройки: venue.ws_feed_url, venue.fast_book_levels, venue.fast_book_req_per_sec [htx] (решение 422);
//! venue.ws_subs_per_sec, venue.ws_silence_ms, venue.ws_reconnect_pause_ms, venue.depth_levels [htx],
//! http.timeout_ms — общие с соединением снимков (ws.rs).
//!
//! Официально (https://huobiapi.github.io/docs/spot/v1/en/, «Websocket Market Data»): «MBP incremental channel and its
//! REQ channel» — адрес wss://api.huobi.pro/feed или wss://api-aws.huobi.pro/feed (настройка venue.ws_feed_url);
//! уровни темы 5, 20, 150, 400, запрос снимка — только 5, 20, 150 (без снимка синхронизации нет — 400 не берём);
//! «5-level/20-level incremental MBP is a tick by tick feed» и только для пар из списка документации; 150/400 — все
//! пары, раз в 100 мс; лимит запроса: «The limitation of single connection is 100 ms» (не больше 10 в секунду).

use std::time::Duration;

use tokio::sync::mpsc;

use crate::core::types::Asset;
use crate::gateway::core_out_book::BookOut;
use crate::gateway::events::MarketEvent;
use crate::storage::journal::Level;

use super::book::Books;
use super::ctx::Ctx;
use super::ws::End;
use super::{fast_conn, snap_out, ws};

/// Уровни канала, у которых есть запрос снимка (официально «REQ supports 5, 20, 150»).
pub const FAST_LEVELS: [u64; 3] = [5, 20, 150];
/// Наибольший темп запросов снимка на соединение — официально «100 ms».
pub const REQ_MAX_PER_SEC: u64 = 10;

/// Настройки соединения быстрого канала.
pub struct FastCfg {
    pub url: String,
    /// Уровни канала (5, 20, 150) — и предел локального стакана.
    pub levels: u64,
    /// Уровней ядру (venue.depth_levels — как у снимков).
    pub depth: usize,
    pub sub_gap: Duration,
    pub req_gap: Duration,
    pub silence: Duration,
    /// http.timeout_ms: время на подключение и срок ответа на запрос снимка (решение 422, ревью).
    pub connect_timeout: Duration,
    /// Пауза перед новым запросом снимка после разрыва или ошибки (venue.ws_reconnect_pause_ms — урок 12).
    pub pause: Duration,
}

/// Работает, пока жив канал out: подключается заново после каждого обрыва. Принимает: окружение, имя соединения,
/// монеты (монета, символ HTX), стаканы, канал ядра.
pub async fn run(
    ctx: Ctx,
    name: String,
    assets: Vec<(Asset, String)>,
    books: Books,
    out: mpsc::Sender<MarketEvent>,
) {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let pairs: Vec<(String, String)> = assets
        .iter()
        .map(|(a, s)| (a.ticker.clone(), s.clone()))
        .collect();
    books.mode.resyncs().register(&pairs, &tx);
    while !out.is_closed() {
        let end = match config(&ctx) {
            Ok(Some(cfg)) => {
                books.mode.set_levels(cfg.levels);
                fast_conn::session(&ctx, &name, &assets, &books, &out, &cfg, &mut rx).await
            }
            Ok(None) => End::Config("venue.ws_feed_url не задана".into()),
            Err(e) => End::Config(e),
        };
        let why = match &end {
            End::Done => return,
            End::Broken(r) | End::Config(r) => format!("быстрый канал {name}: {r}"),
        };
        let kept = BookOut::new(&out, ctx.venue(), &ctx.settings, &ctx.journal);
        for (a, _) in &assets {
            if !snap_out::back(&ctx, &books, &out, &kept, &a.ticker, &why) {
                return;
            }
        }
        books.mode.log().flush(&ctx).await;
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
            (_, _) => (
                Level::Error,
                format!("WebSocket {name} остановлен (пары на снимках mbp.refresh): {why}"),
                None,
            ),
        };
        ctx.log(level, "venue.ws", text).await;
        match pause {
            Some(ms) => tokio::time::sleep(Duration::from_millis(ms)).await,
            None => {
                books.mode.set_levels(0);
                return;
            }
        }
    }
}

/// Читает настройки быстрого канала. Возвращает: None — venue.ws_feed_url не задана (быстрой ветки нет, только
/// снимки — как до решения 422). Ошибка — текст «не задана …» или «вне пределов документации».
pub fn config(ctx: &Ctx) -> Result<Option<FastCfg>, String> {
    let Ok(url) = ctx.text("venue.ws_feed_url") else {
        return Ok(None);
    };
    let m = |e: crate::gateway::errors::GatewayError| e.message;
    let levels = ctx.int("venue.fast_book_levels").map_err(m)?;
    if !FAST_LEVELS.contains(&levels) {
        return Err(format!(
            "venue.fast_book_levels = {levels}: у HTX запрос снимка канала mbp есть только для {FAST_LEVELS:?}"
        ));
    }
    let req = ctx.int("venue.fast_book_req_per_sec").map_err(m)?;
    if req == 0 || req > REQ_MAX_PER_SEC {
        return Err(format!(
            "venue.fast_book_req_per_sec = {req}: у HTX от 1 до {REQ_MAX_PER_SEC} («The limitation of single connection is 100 ms»)"
        ));
    }
    // Общие с соединением снимков: темп подписок, тишина, уровни ядру, таймаут — их проверяет ws::config.
    let snap = ws::config(ctx)?;
    let pause = ctx.int("venue.ws_reconnect_pause_ms").map_err(m)?;
    Ok(Some(FastCfg {
        url,
        levels,
        depth: snap.levels as usize,
        sub_gap: snap.sub_gap,
        req_gap: Duration::from_secs_f64(1.0 / req as f64),
        silence: snap.silence,
        connect_timeout: snap.connect_timeout,
        pause: Duration::from_millis(pause),
    }))
}

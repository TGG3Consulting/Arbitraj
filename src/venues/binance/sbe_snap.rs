//! Файл: venues/binance/sbe_snap.rs
//! Что делает: решение 425 — снимки стакана REST GET /api/v3/depth для локальных стаканов SBE Binance: одна на
//! адаптер очередь с темпом venue.sbe_snapshot_per_sec (старт сотен пар и пересинхронизации после переподключения
//! идут по очереди, а не залпом — вес по IP), глубина — venue.sbe_snapshot_limit. Перед самим запросом в канал
//! соединения уходит Start (пара начинает копить изменения — sbe_book.rs begin), после — снимок или причина сбоя.
//! Кто вызывает: sbe_pair.rs (запрос снимка пары), sbe_run.rs (config — проверка до запуска).
//! От чего зависит: rest.rs (public: пауза после 418/429 по Retry-After — там), depth.rs (разбор уровней), ctx.rs.
//! Настройки: venue.sbe_snapshot_per_sec [binance], venue.sbe_snapshot_limit [binance] (1–5000).
//!
//! Официально (rest-api.md «Order book», github.com/binance/binance-spot-api-docs): GET /api/v3/depth, symbol, limit
//! («Default: 100; Maximum: 5000»), вес по limit: 1-100 — 5, 101-500 — 25, 501-1000 — 50, 1001-5000 — 250; ответ
//! {lastUpdateId, bids, asks}. Лимит веса — REQUEST_WEIGHT по IP (число — в rateLimits живого exchangeInfo); темп
//! очереди × вес снимка в минуту должен оставлять место остальным запросам (совет — docs/FIRST_RUN.md).
//! «How to manage a local order book correctly», пункт 3: снимок — этим запросом.

use std::sync::LazyLock;
use std::time::Duration;

use tokio::sync::{Mutex, mpsc};
use tokio::time::Instant;

use crate::core::types::PriceLevel;

use super::ctx::Ctx;
use super::depth::{self, Partial};
use super::rest;

/// Наибольший limit снимка — официально «Maximum: 5000».
pub const MAX_LIMIT: u64 = 5000;
/// Когда можно слать следующий снимок (один адаптер Binance на процесс — одна очередь на IP).
static NEXT: LazyLock<Mutex<Option<Instant>>> = LazyLock::new(|| Mutex::new(None));

/// Снимок пары в числах ядра.
pub struct Snap {
    pub id: i64,
    pub bids: Vec<PriceLevel>,
    pub asks: Vec<PriceLevel>,
}

/// Сообщение очереди соединению (ключ — символ строчными).
pub enum Done {
    /// Очередь дошла: запрос уходит сейчас — начинать копить изменения.
    Start(String),
    /// Снимок или причина сбоя.
    Snap(String, Result<Snap, String>),
}

/// Что делает: настройки очереди. Возвращает: (пауза между запросами, limit). Ошибка — текст причины (не задана, 0,
/// limit больше 5000).
pub fn config(ctx: &Ctx) -> Result<(Duration, u64), String> {
    let m = |e: crate::gateway::errors::GatewayError| e.message;
    let rate = ctx.int("venue.sbe_snapshot_per_sec").map_err(m)?;
    if rate == 0 {
        return Err("venue.sbe_snapshot_per_sec = 0: нужно от 1".into());
    }
    let limit = ctx.int("venue.sbe_snapshot_limit").map_err(m)?;
    if limit == 0 || limit > MAX_LIMIT {
        return Err(format!(
            "venue.sbe_snapshot_limit = {limit}: у Binance от 1 до {MAX_LIMIT}"
        ));
    }
    Ok((Duration::from_secs_f64(1.0 / rate as f64), limit))
}

/// Что делает: задача снимка пары — ждёт очереди, шлёт Start, запрашивает, разбирает, шлёт итог.
/// Принимает: окружение, символ строчными (ключ), символ Binance (для запроса), канал соединения. Ошибок нет (сбой —
/// в итоге).
pub fn request(ctx: &Ctx, key: String, symbol: String, out: mpsc::Sender<Done>) {
    let ctx = ctx.clone();
    tokio::spawn(async move {
        let res = fetch(&ctx, &key, &symbol, &out).await;
        let _ = out.send(Done::Snap(key, res)).await;
    });
}

/// Очередь, Start, запрос и разбор одного снимка. Ошибка — текст причины.
async fn fetch(
    ctx: &Ctx,
    key: &str,
    symbol: &str,
    out: &mpsc::Sender<Done>,
) -> Result<Snap, String> {
    let (gap, limit) = config(ctx)?;
    {
        let mut next = NEXT.lock().await;
        if let Some(at) = *next {
            tokio::time::sleep_until(at).await;
        }
        // Ревью 425: соединение уже закрыто (обрыв, пока ждали очереди) — место в очереди не занимаем, запроса нет.
        if out.is_closed() {
            return Err("соединение закрыто".into());
        }
        *next = Some(Instant::now() + gap);
    }
    if out.send(Done::Start(key.to_string())).await.is_err() {
        return Err("соединение закрыто".into());
    }
    let q = vec![("symbol", symbol.to_string()), ("limit", limit.to_string())];
    let v = rest::public(ctx, "/api/v3/depth", q)
        .await
        .map_err(|e| e.message)?;
    let p: Partial<'_> = serde::Deserialize::deserialize(&v)
        .map_err(|e| format!("снимок {symbol} не разобран: {e}"))?;
    let (bids, asks) = depth::levels(&p)?;
    Ok(Snap {
        id: p.last_update_id,
        bids,
        asks,
    })
}

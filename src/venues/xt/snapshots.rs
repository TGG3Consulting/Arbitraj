//! Файл: venues/xt/snapshots.rs
//! Что делает: снимки стакана REST /v4/public/depth для локальных стаканов XT.com — общая на все соединения очередь
//! с темпом venue.depth_snapshot_per_sec (у XT 10 запросов в секунду на IP): старт сотен пар и ресинки идут по
//! очереди, а не залпом (бан IP после 429). Снимок разбирается в числа ядра и отдаётся в канал своего соединения.
//! Дополнение 10.11 к решениям 202/213 (Тигран: согласовано): снимок пары и при переподписке сторожем ядра — в той же
//! очереди (ws_book.rs resub): depth_update — limit = venue.depth_snapshot_limit (стакан пары заново со снимка), depth —
//! limit = venue.depth_levels (столько уровней даёт кадр depth@).
//! Кто вызывает: ws_diff.rs (запрос снимка пары), ws_book.rs (переподписка), ws.rs (config).
//! От чего зависит: rest.rs, depth.rs, ctx.rs.
//! Настройки: venue.depth_snapshot_per_sec [xt] (1–10), venue.depth_snapshot_limit [xt] (1–500; не нужна снимку при
//! переподписке в канале depth).
//!
//! Официально (https://doc.xt.com/docs/spot/Market/GetDepthData, `_market/3depth.md`): GET /v4/public/depth?symbol=&limit= («1~500»),
//! «Limit Flow Rules 10/s/ip»; result {timestamp, lastUpdateId, bids, asks}. Инструкция «Orderbook manage» советует
//! limit=500.

use std::sync::LazyLock;
use std::time::Duration;

use tokio::sync::{Mutex, mpsc};
use tokio::time::Instant;

use crate::core::types::PriceLevel;

use super::ctx::Ctx;
use super::depth::{RestDepth, convert};
use super::rest;

/// Наибольший темп снимков — официально «10/s/ip».
pub const MAX_PER_SEC: u64 = 10;
/// Наибольший limit снимка — официально «1~500».
pub const MAX_LIMIT: u64 = 500;
/// Когда можно слать следующий снимок (один адаптер XT на процесс — одна очередь на IP).
static NEXT: LazyLock<Mutex<Option<Instant>>> = LazyLock::new(|| Mutex::new(None));

/// Снимок пары в числах ядра.
pub struct Snap {
    pub id: i64,
    pub bids: Vec<PriceLevel>,
    pub asks: Vec<PriceLevel>,
}

/// Итог запроса снимка: символ и снимок или причина сбоя.
pub type Done = (String, Result<Snap, String>);

/// Что делает: настройки очереди снимков. Возвращает: (пауза между запросами, limit). Ошибка — текст причины.
pub fn config(ctx: &Ctx) -> Result<(Duration, u64), String> {
    let m = |e: crate::gateway::errors::GatewayError| e.message;
    let gap = pace(ctx)?;
    let limit = ctx.int("venue.depth_snapshot_limit").map_err(m)?;
    if limit == 0 || limit > MAX_LIMIT {
        return Err(format!(
            "venue.depth_snapshot_limit = {limit}: у XT от 1 до {MAX_LIMIT}"
        ));
    }
    Ok((gap, limit))
}

/// Что делает: темп очереди — пауза между снимками (venue.depth_snapshot_per_sec, 1–10). Ошибка — текст причины.
pub fn pace(ctx: &Ctx) -> Result<Duration, String> {
    let rate = ctx
        .int("venue.depth_snapshot_per_sec")
        .map_err(|e| e.message)?;
    if rate == 0 || rate > MAX_PER_SEC {
        return Err(format!(
            "venue.depth_snapshot_per_sec = {rate}: у XT от 1 до {MAX_PER_SEC} («10/s/ip» — на все публичные запросы стакана)"
        ));
    }
    Ok(Duration::from_secs_f64(1.0 / rate as f64))
}

/// Что делает: запускает задачу снимка пары: ждёт своей очереди, запрашивает, разбирает и шлёт итог в канал.
/// Принимает: окружение, символ XT, limit (None — venue.depth_snapshot_limit; Some — снимок канала depth при
/// переподписке, venue.depth_levels), канал соединения. Ошибок нет (сбой — в итоге).
pub fn request(ctx: &Ctx, symbol: String, limit: Option<u64>, out: mpsc::Sender<Done>) {
    let ctx = ctx.clone();
    tokio::spawn(async move {
        let res = fetch(&ctx, &symbol, limit).await;
        let _ = out.send((symbol, res)).await;
    });
}

/// Очередь, запрос и разбор одного снимка. Ошибка — текст причины.
async fn fetch(ctx: &Ctx, symbol: &str, limit: Option<u64>) -> Result<Snap, String> {
    let (gap, limit) = match limit {
        Some(n) if (1..=MAX_LIMIT).contains(&n) => (pace(ctx)?, n),
        Some(n) => return Err(format!("limit {n}: у XT от 1 до {MAX_LIMIT}")),
        None => config(ctx)?,
    };
    {
        let mut next = NEXT.lock().await;
        if let Some(at) = *next {
            tokio::time::sleep_until(at).await;
        }
        *next = Some(Instant::now() + gap);
    }
    let q = vec![("symbol", symbol.to_string()), ("limit", limit.to_string())];
    let v = rest::public(ctx, "/v4/public/depth", q)
        .await
        .map_err(|e| e.message)?;
    let d: RestDepth<'_> = serde::Deserialize::deserialize(&v)
        .map_err(|e| format!("снимок {symbol} не разобран: {e}"))?;
    Ok(Snap {
        id: d.last_update_id,
        bids: convert(&d.bids, "bids")?,
        asks: convert(&d.asks, "asks")?,
    })
}

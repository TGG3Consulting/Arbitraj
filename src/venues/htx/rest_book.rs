//! Файл: venues/htx/rest_book.rs
//! Что делает: решение 237 — снимок стакана пары HTX по REST для ордера (контракт quote_rest), когда стакан потока
//! mbp.refresh устарел (кадр пары старше venue.book_stale_ms) или его нет: один публичный запрос GET /market/depth.
//! Кто вызывает: mod.rs (quote_rest).
//! От чего зависит: rest.rs (public; reply.rs — у Market Data полезная часть в tick, пауза после 429), depth.rs (levels
//! — разбор уровней [цена, количество] числами, как у кадра mbp.refresh), ws.rs (DEPTH_LEVELS), ctx.rs.
//! Настройки: venue.depth_levels [htx] (depth — столько уровней даёт кадр mbp.refresh), адрес REST — rest.rs.
//!
//! Официально: Spot API v1, Market Data → «Get Market Depth» (https://huobiapi.github.io/docs/spot/v1/en/
//! #get-market-depth): «GET /market/depth», symbol (обязателен, «Refer to GET /v1/common/symbols»), depth («The number
//! of market depth to return on each side», 5, 10, 20; по умолчанию 20, при step0 — 150), type (обязателен, step0 —
//! без объединения цен); ответ — status, ch, ts и tick {ts, version («Internal data»), bids, asks} уровнями
//! [price, size] («The data is under the tick object, not data»). Порядок уровней в описании не оговорён — сортировка
//! от лучшей цены (gateway/rest_book.rs). Лимит: строки лимита у раздела нет («Only those endpoints marked with rate
//! limit value separately are applied with new rate limit rule»); ответ на превышение не описан — пауза по HTTP 429
//! (reply.rs).

use serde_json::Value;

use crate::core::types::{Asset, OrderBook};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::Ctx;
use super::depth::levels;
use super::rest;
use super::ws::DEPTH_LEVELS;

/// Путь стакана REST.
const PATH: &str = "/market/depth";

/// Снимок стакана пары. Принимает: окружение, символ HTX пары (справочник), монету (тикер HTX).
/// Возвращает: стакан (все уровни ответа, от лучшей цены; время биржи — tick.ts). Ошибки: NotConfigured —
/// venue.depth_levels не задана или не 5/10/20, адрес REST; связь, RateLimited, коды HTX (rest.rs); BadResponse — вид.
pub async fn snapshot(ctx: &Ctx, symbol: &str, asset: &Asset) -> GatewayResult<OrderBook> {
    let n = ctx.int("venue.depth_levels")?;
    if !DEPTH_LEVELS.contains(&n) {
        return Err(ctx.tag(GatewayError::new(
            ErrorCode::NotConfigured,
            None,
            format!("venue.depth_levels = {n}: у HTX стакан REST только depth {DEPTH_LEVELS:?}"),
        )));
    }
    let q = vec![
        ("symbol", symbol.to_string()),
        ("depth", n.to_string()),
        ("type", "step0".to_string()),
    ];
    let tick = rest::public(ctx, PATH, q).await?;
    parse(ctx, &tick, asset)
}

/// Стакан из tick ответа. Принимает: окружение, tick, монету. Ошибки: BadResponse — tick нет, уровень не [цена,
/// количество] или не число.
pub fn parse(ctx: &Ctx, tick: &Value, asset: &Asset) -> GatewayResult<OrderBook> {
    let bad = |why: String| {
        ctx.tag(GatewayError::new(
            ErrorCode::BadResponse,
            None,
            format!("HTX {PATH} {}: {why}", asset.ticker),
        ))
    };
    if !tick.is_object() {
        return Err(bad("нет tick".into()));
    }
    let bids = levels(tick, "bids").map_err(bad)?;
    let asks = levels(tick, "asks").map_err(bad)?;
    let ts = crate::gateway::rest_book::ms(tick.get("ts").and_then(Value::as_i64));
    let (venue, asset) = (ctx.venue(), asset.clone());
    Ok(crate::gateway::rest_book::book(
        venue, asset, bids, asks, ts,
    ))
}

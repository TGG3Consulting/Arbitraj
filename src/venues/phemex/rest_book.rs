//! Файл: venues/phemex/rest_book.rs
//! Что делает: решение 237 — снимок стакана пары Phemex по REST для ордера (контракт quote_rest), когда стакан потока
//! (orderbook.subscribe) устарел (кадр пары старше venue.book_stale_ms) или его нет: один публичный запрос
//! GET /md/orderbook.
//! Кто вызывает: mod.rs (quote_rest).
//! От чего зависит: rest.rs (public — ответ /md… отдаётся полем result, reply.rs; пауза после 429), depth.rs (levels —
//! [priceEp, size] целыми, как у кадра стакана), num.rs (unscale), symbols.rs (SymbolRules: symbol, priceScale,
//! valueScale базовой — те же масштабы, что у кадра в ws_conn.rs), ctx.rs.
//! Настройки: адрес REST — rest.rs (глубина REST у Phemex не задаётся: «depth»: 30 в ответе).
//!
//! Официально: Spot REST API → Market Data API List → «Query Order Book» (github.com/phemex/phemex-api-docs,
//! Public-Spot-API-en.md, #queryorderbook): «GET /md/orderbook?symbol=<symbol>»; ответ {"error": null, "id": 0,
//! "result": {"book": {"asks": [[<priceEp>, <size>]], "bids": […]}, "depth": 30, "sequence", "timestamp", "symbol",
//! "type": "snapshot"}}; priceEp — «Scaled book level price», size — «Scaled book level size», timestamp —
//! «Timestamp in nanoseconds». Лимит (Generic-API-Info.en.md): «Phemex restrict every IP 5,000 requests in 5-minute
//! window»; HTTP 429 — превышение (Public-Spot-API-en.md). Порядок уровней не оговорён — сортировка от лучшей цены
//! (gateway/rest_book.rs).

use serde_json::Value;

use crate::core::types::{Asset, OrderBook, PriceLevel};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::Ctx;
use super::depth::{RawLevel, levels};
use super::symbols::SymbolRules;
use super::{num, rest};

/// Путь стакана REST.
const PATH: &str = "/md/orderbook";

/// Снимок стакана пары. Принимает: окружение, правила пары (символ и масштабы), монету (тикер Phemex).
/// Возвращает: стакан (все уровни ответа, от лучшей цены; время биржи — timestamp ответа). Ошибки: адрес REST, связь,
/// RateLimited, коды Phemex (rest.rs); BadResponse — вид ответа.
pub async fn snapshot(ctx: &Ctx, rules: &SymbolRules, asset: &Asset) -> GatewayResult<OrderBook> {
    let q = vec![("symbol", rules.symbol.clone())];
    let result = rest::public(ctx, PATH, q, false).await?;
    parse(ctx, &result, rules, asset)
}

/// Стакан из result ответа. Принимает: окружение, result, правила пары, монету. Ошибки: BadResponse — нет book,
/// уровень не [priceEp, size] целыми ≥ 0 или не ложится на масштаб.
pub fn parse(
    ctx: &Ctx,
    result: &Value,
    rules: &SymbolRules,
    asset: &Asset,
) -> GatewayResult<OrderBook> {
    let bad = |why: String| {
        ctx.tag(GatewayError::new(
            ErrorCode::BadResponse,
            None,
            format!("Phemex {PATH} {}: {why}", asset.ticker),
        ))
    };
    let book = result
        .get("book")
        .ok_or_else(|| bad("нет result.book".into()))?;
    let side = |k: &str| -> Result<Vec<PriceLevel>, String> {
        levels(book, k)?
            .iter()
            .map(|l: &RawLevel| unscale(l, rules, k))
            .collect()
    };
    let bids = side("bids").map_err(bad)?;
    let asks = side("asks").map_err(bad)?;
    let ts = result
        .get("timestamp")
        .and_then(Value::as_i64)
        .map(chrono::DateTime::<chrono::Utc>::from_timestamp_nanos);
    let (venue, asset) = (ctx.venue(), asset.clone());
    Ok(crate::gateway::rest_book::book(
        venue, asset, bids, asks, ts,
    ))
}

/// Уровень [priceEp, size] → цена и количество (priceScale, valueScale базовой). Ошибка — текст.
fn unscale(l: &RawLevel, rules: &SymbolRules, k: &str) -> Result<PriceLevel, String> {
    let price = num::unscale(i128::from(l.0), rules.price_scale)
        .ok_or_else(|| format!("{k}: цена {} вне масштаба {}", l.0, rules.price_scale))?;
    let qty = num::unscale(i128::from(l.1), rules.base_scale)
        .ok_or_else(|| format!("{k}: количество {} вне масштаба {}", l.1, rules.base_scale))?;
    Ok(PriceLevel { price, qty })
}

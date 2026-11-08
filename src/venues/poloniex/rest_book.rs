//! Файл: venues/poloniex/rest_book.rs
//! Что делает: решение 237 — снимок стакана пары Poloniex по REST для ордера (контракт quote_rest), когда стакан потока
//! (канал book) устарел (кадр пары старше venue.book_stale_ms) или его нет: один публичный запрос
//! GET /markets/{symbol}/orderBook.
//! Кто вызывает: mod.rs (quote_rest).
//! От чего зависит: rest.rs (public — тело ответа, пауза после 429 — reply.rs), ws.rs (DEPTH_LEVELS), ctx.rs.
//! Настройки: venue.depth_levels [poloniex] (limit — столько уровней даёт канал book), адрес REST — rest.rs.
//!
//! Официально: Spot → Public → Market Data → «Order Book» (https://api-docs.poloniex.com/spot/api/public/market-data):
//! «GET https://api.poloniex.com/markets/{symbol}/orderBook», scale (необязателен; не задан — «no grouping or
//! aggregation» — не шлём), limit («Valid limit values are: 5, 10, 20, 50, 100, 150», по умолчанию 10; у канала book —
//! 5, 10, 20, поэтому limit = venue.depth_levels); ответ — time, scale, asks («List<String>», «list of asks»), bids,
//! ts. Вид списка: описание поля его не раскрывает, пример ответа — плоский список строк «цена, количество, цена,
//! количество…» (bids ["1.9", "110", "1.889", "10"]) — разбор по примеру; иной вид (нечётная длина, не строка, не
//! число) — BadResponse, сделка не идёт (вопрос — docs/TASKS.md). Лимит: в таблице лимитов строка Order Book без
//! значения («The market-related interfaces are frequency limited based on IP latitude», https://api-docs.poloniex.com/
//! spot/api/). Порядок уровней не оговорён — сортировка от лучшей цены (gateway/rest_book.rs).

use std::str::FromStr;

use rust_decimal::Decimal;
use serde_json::Value;

use crate::core::types::{Asset, OrderBook, PriceLevel};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::Ctx;
use super::rest;
use super::ws::DEPTH_LEVELS;

/// Снимок стакана пары. Принимает: окружение, символ Poloniex пары (справочник), монету (тикер Poloniex).
/// Возвращает: стакан (все уровни ответа, от лучшей цены; время биржи — time ответа). Ошибки: NotConfigured —
/// venue.depth_levels не задана или не 5/10/20, адрес REST; связь, RateLimited, коды Poloniex; BadResponse — вид.
pub async fn snapshot(ctx: &Ctx, symbol: &str, asset: &Asset) -> GatewayResult<OrderBook> {
    let n = ctx.int("venue.depth_levels")?;
    if !DEPTH_LEVELS.contains(&n) {
        return Err(ctx.tag(GatewayError::new(
            ErrorCode::NotConfigured,
            None,
            format!("venue.depth_levels = {n}: у канала book Poloniex только {DEPTH_LEVELS:?}"),
        )));
    }
    let path = format!("/markets/{symbol}/orderBook");
    let v = rest::public(ctx, &path, vec![("limit", n.to_string())]).await?;
    parse(ctx, &v, asset)
}

/// Стакан из ответа. Принимает: окружение, ответ, монету. Ошибки: BadResponse — нет asks / bids, список не из пар
/// «цена, количество» строками.
pub fn parse(ctx: &Ctx, v: &Value, asset: &Asset) -> GatewayResult<OrderBook> {
    let bad = |why: String| {
        ctx.tag(GatewayError::new(
            ErrorCode::BadResponse,
            None,
            format!("Poloniex orderBook {}: {why}", asset.ticker),
        ))
    };
    let bids = side(v, "bids").map_err(bad)?;
    let asks = side(v, "asks").map_err(bad)?;
    let ts = crate::gateway::rest_book::ms(v.get("time").and_then(Value::as_i64));
    let (venue, asset) = (ctx.venue(), asset.clone());
    Ok(crate::gateway::rest_book::book(
        venue, asset, bids, asks, ts,
    ))
}

/// Одна сторона: плоский список «цена, количество, …» строками. Ошибка — текст причины.
fn side(v: &Value, k: &str) -> Result<Vec<PriceLevel>, String> {
    let arr = v
        .get(k)
        .and_then(Value::as_array)
        .ok_or_else(|| format!("нет {k}"))?;
    if arr.len() % 2 != 0 {
        return Err(format!(
            "{k}: нечётная длина {} — не пары «цена, количество»",
            arr.len()
        ));
    }
    let num = |x: &Value, what: &str| {
        x.as_str()
            .and_then(|s| Decimal::from_str(s.trim()).ok())
            .ok_or_else(|| format!("{k}: {what} {x} не число строкой"))
    };
    arr.chunks(2)
        .map(|p| {
            Ok(PriceLevel {
                price: num(&p[0], "цена")?,
                qty: num(&p[1], "количество")?,
            })
        })
        .collect()
}

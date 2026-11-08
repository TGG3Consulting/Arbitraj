//! Файл: venues/kraken/rest_book.rs
//! Что делает: решение 237 — снимок стакана пары Kraken по REST для ордера (контракт quote_rest), когда стакан потока
//! (WS v2 book) устарел (кадр пары старше venue.book_stale_ms) или его нет: один публичный запрос GET /0/public/Depth.
//! Кто вызывает: mod.rs (quote_rest).
//! От чего зависит: rest.rs (public — {error, result}, паузы после «Rate limit exceeded» — reply.rs), ctx.rs.
//! Настройки: venue.depth_levels [kraken] (count — столько уровней даёт канал book, но не больше 500 — предел
//! документации), адрес REST — rest.rs.
//!
//! Официально: Spot REST → Market Data → «Get Order Book» (https://docs.kraken.com/api-reference/market-data/
//! get-order-book.md): «get /public/Depth», pair (обязателен), count («Maximum number of asks/bids», 1–500, по
//! умолчанию 100); ответ result — объект, ключ — имя пары (без assetVersion — внутреннее имя Kraken, например XXBTZUSD,
//! может не совпадать с запрошенным), значение — asks / bids («array of entries [<price>, <volume>, <timestamp>]», ровно
//! 3 элемента; в примере цена и объём — строки, время — целое). Порядок уровней в описании не оговорён — сортировка от
//! лучшей цены (gateway/rest_book.rs). Лимит публичных запросов на странице метода не указан.

use std::str::FromStr;

use rust_decimal::Decimal;
use serde_json::Value;

use crate::core::types::{Asset, OrderBook, PriceLevel};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::Ctx;
use super::rest;

/// Метод REST.
const METHOD: &str = "Depth";
/// Наибольший count — официально «maximum: 500» (Get Order Book).
const MAX_COUNT: u64 = 500;

/// Снимок стакана пары. Принимает: окружение, имя пары Kraken для REST (справочник, ключ AssetPairs), монету (тикер
/// Kraken). Возвращает: стакан (все уровни ответа, от лучшей цены; времени книги в ответе нет — у уровней своё время).
/// Ошибки: NotConfigured (venue.depth_levels, адрес REST), связь, RateLimited, error[] Kraken; BadResponse — вид.
pub async fn snapshot(ctx: &Ctx, pair: &str, asset: &Asset) -> GatewayResult<OrderBook> {
    let n = ctx.int("venue.depth_levels")?.min(MAX_COUNT);
    let q = vec![
        ("pair".to_string(), pair.to_string()),
        ("count".to_string(), n.to_string()),
    ];
    let result = rest::public(ctx, METHOD, q).await?;
    parse(ctx, &result, asset)
}

/// Стакан из result ответа: одна пара (запрошена одна) — её asks / bids. Ошибки: BadResponse — result не объект из
/// одной пары, уровень не [цена, объём, время] или не число.
pub fn parse(ctx: &Ctx, result: &Value, asset: &Asset) -> GatewayResult<OrderBook> {
    let bad = |why: String| {
        ctx.tag(GatewayError::new(
            ErrorCode::BadResponse,
            None,
            format!("Kraken /0/public/{METHOD} {}: {why}", asset.ticker),
        ))
    };
    let pairs = result
        .as_object()
        .ok_or_else(|| bad("result не объект".into()))?;
    if pairs.len() != 1 {
        return Err(bad(format!("в result {} пар, запрошена одна", pairs.len())));
    }
    let Some(book) = pairs.values().next() else {
        return Err(bad("result пуст".into()));
    };
    let bids = side(book, "bids").map_err(bad)?;
    let asks = side(book, "asks").map_err(bad)?;
    let (venue, asset) = (ctx.venue(), asset.clone());
    Ok(crate::gateway::rest_book::book(
        venue, asset, bids, asks, None,
    ))
}

/// Одна сторона: [[цена, объём, время], …]; цена и объём — строки (или числа, схема допускает). Ошибка — текст.
fn side(book: &Value, k: &str) -> Result<Vec<PriceLevel>, String> {
    let arr = book
        .get(k)
        .and_then(Value::as_array)
        .ok_or_else(|| format!("нет {k}"))?;
    let num = |x: &Value, what: &str| -> Result<Decimal, String> {
        let t = match x {
            Value::String(s) => s.trim().to_string(),
            Value::Number(n) => n.to_string(),
            _ => String::new(),
        };
        Decimal::from_str(&t).map_err(|_| format!("{k}: {what} {x} не число"))
    };
    arr.iter()
        .map(|l| {
            let p = l
                .as_array()
                .filter(|p| p.len() == 3)
                .ok_or_else(|| format!("{k}: уровень не [цена, объём, время]"))?;
            Ok(PriceLevel {
                price: num(&p[0], "цена")?,
                qty: num(&p[1], "объём")?,
            })
        })
        .collect()
}

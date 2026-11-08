//! Файл: venues/bitget/rest_book.rs
//! Что делает: решение 201 — свежий снимок стакана пары по REST для котировки ИСПОЛНЕНИЯ (оценка продажи перед ордером,
//! «Продать сейчас»), когда стакана потока нет или кадр пары старше venue.book_stale_ms. Причина (06.11.2026): RWA на
//! Bitget при живом соединении не получала кадров books5/books15 45–55 с — «Продать сейчас» раз за разом отказывала
//! «кадра стакана пары нет». Поток котировок расчётов снимком не пользуется — только исполнение.
//! Кто вызывает: mod.rs (quote при gateway/patience.rs is_exec).
//! От чего зависит: rest.rs (public), depth.rs (levels — разбор уровней [цена, количество]), ctx.rs, core/types.
//! Настройки: venue.depth_levels [bitget] — сколько уровней просить (limit).
//!
//! Официально: «Get OrderBook Depth», GET /api/v2/spot/market/orderbook, «20 times/1s (IP)», параметры symbol, type
//! (step0…step5 — step0 без объединения цен), limit (≤ 150) — раздел Spot → Market Classic v2
//! (https://www.bitget.com/docs/catalog/classic-spot-market/classic-spot-market; выписка — docs/notes/
//! bitget-analysis-2026-11-05.md, раздел 5). Поля ответа data — asks / bids уровнями [цена, количество] (как кадр Depth
//! Channel) и ts; разбор строгий: иной вид — BadResponse с причиной, ответ — в сыром журнале (rest.rs), сделка не идёт
//! по догадке.

use chrono::{TimeZone, Utc};
use serde_json::Value;

use crate::core::types::{Asset, OrderBook};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::Ctx;
use super::{depth, rest};

/// Путь снимка стакана.
pub(super) const PATH: &str = "/api/v2/spot/market/orderbook";

/// Снимок стакана пары. Принимает: окружение, символ Bitget, монету. Возвращает: стакан (asks по возрастанию цены, bids
/// по убыванию). Ошибки: NotConfigured (venue.depth_levels), связь и коды Bitget (rest.rs), BadResponse — вид ответа.
pub async fn snapshot(ctx: &Ctx, symbol: &str, asset: &Asset) -> GatewayResult<OrderBook> {
    let limit = ctx.int("venue.depth_levels")?;
    let q = vec![
        ("symbol", symbol.to_string()),
        ("type", "step0".to_string()),
        ("limit", limit.to_string()),
    ];
    let data = rest::public(ctx, PATH, q).await?;
    parse(ctx, &data, asset)
}

/// Разбор data ответа. Ошибки: BadResponse — нет asks / bids или уровень не [цена, количество].
pub fn parse(ctx: &Ctx, data: &Value, asset: &Asset) -> GatewayResult<OrderBook> {
    let bad = |why: String| {
        ctx.tag(GatewayError::new(
            ErrorCode::BadResponse,
            None,
            format!("Bitget {PATH} {}: {why}", asset.ticker),
        ))
    };
    let mut asks = depth::levels(data, "asks").map_err(bad)?;
    let mut bids = depth::levels(data, "bids").map_err(bad)?;
    asks.sort_by_key(|a| a.price);
    bids.sort_by_key(|b| std::cmp::Reverse(b.price));
    let ts = match data.get("ts") {
        Some(Value::String(s)) => s.parse::<i64>().ok(),
        Some(x) => x.as_i64(),
        None => None,
    };
    Ok(OrderBook {
        venue: ctx.venue(),
        asset: asset.clone(),
        bids,
        asks,
        exchange_time: ts.and_then(|ms| Utc.timestamp_millis_opt(ms).single()),
        received_at: Utc::now(),
    })
}

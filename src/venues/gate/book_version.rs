//! Файл: venues/gate/book_version.rs
//! Что делает: решение 213 — версия стакана пары Gate по REST для сторожа ядра (контракт book_version): один
//! публичный запрос GET /spot/order_book?currency_pair=…&with_id=true, из ответа — только id, update, current.
//! Решение 237: тот же запрос с уровнями — снимок стакана пары для ордера (snapshot, контракт quote_rest), когда стакан
//! потока устарел: GET /spot/order_book (currency_pair, limit = venue.depth_levels — столько уровней даёт кадр
//! spot.order_book; limit — «Maximum number of order depth data in asks or bids»), уровни asks / bids [цена, количество]
//! строками разбираются так же, как кадр (depth.rs levels); живой ответ — tests/data/cex_samples_2026_11_04/
//! gate_spot_order_book_CAKE_USDT.json.
//! Кто вызывает: mod.rs (book_version, quote_rest).
//! От чего зависит: rest.rs (public — пауза после 429 соблюдается там), ctx.rs (символ пары), gateway/feed_health.rs.
//! Настройки: venue.quote_asset [gate] (символ пары), venue.rest_url и прочие настройки REST (rest.rs).
//!
//! Официально: API v4 https://www.gate.com/docs/developers/apiv4/en/ — Spot, «Retrieve order book» (GET
//! /spot/order_book; параметры currency_pair, interval, limit, with_id — «Return order book update ID»), модель
//! OrderBook (официальный SDK github.com/gate/gateapi-python, docs/OrderBook.md, сгенерирован из спецификации API):
//! - id — «Order book ID, which is updated whenever the order book is changed. Valid only when with_id is set to true»;
//! - current — «The timestamp of the response data being generated (in milliseconds)»;
//! - update — «The timestamp of when the orderbook last changed (in milliseconds)».
//!
//! Глубина (limit) не передаётся — по документации по умолчанию 10 уровней; уровни не читаются.
//! Лимит частоты: API v4 → «Frequency limit rule» — публичные конечные точки 200 запросов за 10 с на IP (см. reply.rs);
//! темп подтверждений задаёт ядро (venue.quiet_confirm_max_per_s ≤ 20 с запасом на другие публичные запросы Gate).
//!
//! Сравнение с кадром (core/feed_confirm.rs): номер id ответа — на той же шкале, что lastUpdateId кадра
//! spot.order_book (ws_conn.rs передаёт его в ConnHandle::frame_ver). Основание: документация Spot WebSocket v4
//! (https://www.gate.com/docs/developers/apiv4/ws/en/, spot.order_book_update) велит вести локальный стакан от
//! снимка REST with_id=true — его id (baseID) сравнивается с U/u кадров изменений, т.е. id REST и u — один счётчик;
//! в живой записи 04.11 (tests/data/cex_samples_2026_11_04/gate_ws.jsonl) lastUpdateId каждого кадра spot.order_book
//! равен u кадра spot.order_book_update с тем же t. Прямой фразы «lastUpdateId = id REST» в документации нет — вопрос
//! Тиграну (docs/TASKS.md). Время кадра t в модель свежести не передаётся (last_changed_ms = None): документация не
//! определяет t как время изменения книги; время update/current используется только для пары без кадров с подписки
//! (возраст книги current − update — без сравнения часов бота и Gate).

use serde_json::Value;

use crate::core::types::{Asset, OrderBook};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::gateway::feed_health::BookVersion;

use super::ctx::Ctx;
use super::rest;

/// Версия стакана пары. Принимает: окружение, тикер Gate (решение 211). Возвращает: id, update, current ответа (поля,
/// которых нет, — None). Ошибки: NotConfigured (venue.quote_asset, адрес REST), связь, RateLimited, коды Gate.
pub async fn book_version(ctx: &Ctx, ticker: &str) -> GatewayResult<BookVersion> {
    let p = vec![
        ("currency_pair", ctx.symbol(ticker)?),
        ("with_id", "true".to_string()),
    ];
    let v = rest::public(ctx, "/spot/order_book", p).await?;
    Ok(parse(&v))
}

/// Решение 237: снимок стакана пары по REST для ордера — GET /spot/order_book (currency_pair, limit =
/// venue.depth_levels; раздел и лимит — в шапке). Принимает: окружение, монету (тикер Gate). Возвращает: стакан (все
/// уровни ответа, от лучшей цены; время биржи — current ответа). Ошибки: NotConfigured (venue.depth_levels,
/// venue.quote_asset, адрес REST), связь, RateLimited, коды Gate; BadResponse — вид ответа.
pub async fn snapshot(ctx: &Ctx, asset: &Asset) -> GatewayResult<OrderBook> {
    let n = ctx.int("venue.depth_levels")?;
    let p = vec![
        ("currency_pair", ctx.symbol(&asset.ticker)?),
        ("limit", n.to_string()),
    ];
    let v = rest::public(ctx, "/spot/order_book", p).await?;
    rest_book(ctx, &v, asset)
}

/// Решение 237: стакан из ответа GET /spot/order_book (уровни — depth.rs levels, время биржи — current).
/// Принимает: окружение, ответ, монету. Ошибки: BadResponse — нет asks / bids или уровень не [цена, количество].
pub fn rest_book(ctx: &Ctx, v: &Value, asset: &Asset) -> GatewayResult<OrderBook> {
    let bad = |why: String| {
        ctx.tag(GatewayError::new(
            ErrorCode::BadResponse,
            None,
            format!("Gate /spot/order_book {}: {why}", asset.ticker),
        ))
    };
    let bids = super::depth::levels(v, "bids").map_err(bad)?;
    let asks = super::depth::levels(v, "asks").map_err(bad)?;
    let ts = crate::gateway::rest_book::ms(v.get("current").and_then(Value::as_i64));
    let (venue, asset) = (ctx.venue(), asset.clone());
    Ok(crate::gateway::rest_book::book(
        venue, asset, bids, asks, ts,
    ))
}

/// Разбор ответа GET /spot/order_book (модель OrderBook). Ошибок нет: нет поля — None.
pub fn parse(v: &Value) -> BookVersion {
    let int = |k: &str| v.get(k).and_then(Value::as_i64);
    BookVersion {
        id: int("id"),
        changed_ms: int("update"),
        server_ms: int("current"),
        // Gate подтверждает по номеру id (не по содержимому).
        print: None,
    }
}

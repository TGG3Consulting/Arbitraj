//! Файл: venues/gate/book_version.rs
//! Что делает: решение 213 — версия стакана пары Gate по REST для сторожа ядра (контракт book_version): один
//! публичный запрос GET /spot/order_book?currency_pair=…&with_id=true, из ответа — только id, update, current.
//! Кто вызывает: mod.rs (book_version).
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

use crate::gateway::errors::GatewayResult;
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

/// Разбор ответа GET /spot/order_book (модель OrderBook). Ошибок нет: нет поля — None.
pub fn parse(v: &Value) -> BookVersion {
    let int = |k: &str| v.get(k).and_then(Value::as_i64);
    BookVersion {
        id: int("id"),
        changed_ms: int("update"),
        server_ms: int("current"),
    }
}

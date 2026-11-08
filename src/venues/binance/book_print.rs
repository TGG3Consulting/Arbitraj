//! Файл: venues/binance/book_print.rs
//! Что делает: решение 213, подтверждение тихой пары Binance ПО СОДЕРЖИМОМУ стакана (Тигран 08.11, решение
//! «202/213 (по содержимому)»; контракт book_version с BookVersion.print): один публичный запрос GET /api/v3/depth
//! (symbol, limit = venue.depth_levels — столько уровней даёт кадр частичного стакана <symbol>@depth<N>[@100ms]), из
//! ответа — первые N уровней bids/asks; после ответа — первые N уровней стакана в памяти (book.rs print — по нему
//! считается котировка). Сравнивает ядро (core/feed_print.rs).
//! Почему не по номеру: у кадра частичного стакана поле lastUpdateId описано только как «Last update ID» (Partial Book
//! Depth Streams); связь с lastUpdateId ответа GET /api/v3/depth документация даёт только для потока изменений
//! <symbol>@depth (U, u — «How to manage a local order book correctly»: «discard any event where u is <= lastUpdateId
//! of the snapshot»). Что lastUpdateId частичного стакана — тот же счётчик, прямо не сказано (решение «202/213
//! (Binance)», вопрос — docs/TASKS.md); номер ответа не используется.
//! Решение 237: тот же запрос и разбор — снимок стакана пары для ордера (snapshot, контракт quote_rest), когда стакан
//! потока устарел: все уровни ответа (limit = venue.depth_levels, тот же предел 5/10/20, вес «1-100: 5»).
//! Кто вызывает: mod.rs (book_version, quote_rest).
//! От чего зависит: rest.rs (public — пауза после 418/429 по Retry-After соблюдается там), depth.rs (разбор уровней),
//! ctx.rs (символ пары, настройки), ws.rs (DEPTH_LEVELS), book.rs (Books::print), gateway/feed_health.rs.
//! Настройки: venue.depth_levels [binance] (N), venue.quote_asset [binance] (символ пары), адрес REST — rest.rs.
//!
//! Официально: REST API → Market Data endpoints → Order book
//! (https://developers.binance.com/docs/binance-spot-api-docs/rest-api/market-data-endpoints#order-book;
//! исходник github.com/binance/binance-spot-api-docs, rest-api.md): «GET /api/v3/depth», параметры symbol (обязателен),
//! limit («Default: 100; Maximum: 5000»); вес по limit — «1-100: 5»; ответ — lastUpdateId, bids и asks («Each entry is
//! [price, quantity]»), пример {"lastUpdateId":1027024,"bids":[["4.00000000","431.00000000"]],"asks":[["4.00000200",
//! "12.00000000"]]}. Порядок уровней в ответе не описан — сортировка от лучшей цены (как кадр, depth.rs levels).
//! Лимит: «The limits on the API are based on the IPs» — вес REQUEST_WEIGHT из exchangeInfo.rateLimits (число —
//! только в живом ответе, docs/notes/binance-analysis-2026-11-05.md §7); темп подтверждений задаёт ядро
//! (venue.quiet_confirm_max_per_s, venue.quiet_confirm_min_ms; совет — docs/FIRST_RUN.md).

use serde_json::Value;

use crate::core::types::{Asset, OrderBook};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::gateway::feed_health::{BookPrint, BookPrints, BookVersion};

use super::book::Books;
use super::ctx::Ctx;
use super::depth::{self, Partial};
use super::rest;
use super::ws::DEPTH_LEVELS;

/// Путь стакана REST.
const PATH: &str = "/api/v3/depth";

/// Отпечатки стакана пары для сторожа ядра. Принимает: окружение, стаканы в памяти, тикер Binance (решение 211).
/// Возвращает: BookVersion с print (id, changed_ms, server_ms — None). Ошибки: NotConfigured — venue.depth_levels не
/// задана или не 5/10/20 (тот же предел, что у кадра, ws.rs), venue.quote_asset, адрес REST; связь, RateLimited, коды
/// Binance (rest.rs); BadResponse — вид ответа.
pub async fn book_version(ctx: &Ctx, books: &Books, ticker: &str) -> GatewayResult<BookVersion> {
    let n = ctx.int("venue.depth_levels")?;
    if !DEPTH_LEVELS.contains(&n) {
        return Err(ctx.tag(GatewayError::new(
            ErrorCode::NotConfigured,
            None,
            format!("venue.depth_levels = {n}: частичный стакан Binance — только {DEPTH_LEVELS:?}"),
        )));
    }
    let p = vec![("symbol", ctx.symbol(ticker)?), ("limit", n.to_string())];
    let v = rest::public(ctx, PATH, p).await?;
    let rest = parse(&v, n as usize).map_err(|why| {
        ctx.tag(GatewayError::new(
            ErrorCode::BadResponse,
            None,
            format!("Binance {PATH} {ticker}: {why}"),
        ))
    })?;
    // Свой стакан — ПОСЛЕ ответа: кадр, пришедший во время запроса, ядро увидит в свежем снимке feed_health.
    let ours = books.print(ticker, n as usize);
    Ok(BookVersion {
        print: Some(BookPrints {
            levels: n as usize,
            rest,
            ours,
        }),
        ..Default::default()
    })
}

/// Решение 237: снимок стакана пары по REST для ордера — GET /api/v3/depth (symbol, limit = venue.depth_levels; раздел,
/// вес — в шапке файла). Принимает: окружение, монету (тикер Binance). Возвращает: стакан (все уровни ответа, от лучшей
/// цены; времени биржи в ответе нет). Ошибки: как book_version.
pub async fn snapshot(ctx: &Ctx, asset: &Asset) -> GatewayResult<OrderBook> {
    let n = ctx.int("venue.depth_levels")?;
    if !DEPTH_LEVELS.contains(&n) {
        return Err(ctx.tag(GatewayError::new(
            ErrorCode::NotConfigured,
            None,
            format!("venue.depth_levels = {n}: частичный стакан Binance — только {DEPTH_LEVELS:?}"),
        )));
    }
    let p = vec![
        ("symbol", ctx.symbol(&asset.ticker)?),
        ("limit", n.to_string()),
    ];
    let v = rest::public(ctx, PATH, p).await?;
    let bad = |why: String| {
        ctx.tag(GatewayError::new(
            ErrorCode::BadResponse,
            None,
            format!("Binance {PATH} {}: {why}", asset.ticker),
        ))
    };
    let p: Partial<'_> =
        serde::Deserialize::deserialize(&v).map_err(|e| bad(format!("ответ не разобран: {e}")))?;
    let (bids, asks) = depth::levels(&p).map_err(bad)?;
    let (venue, asset) = (ctx.venue(), asset.clone());
    Ok(crate::gateway::rest_book::book(
        venue, asset, bids, asks, None,
    ))
}

/// Разбор ответа GET /api/v3/depth: первые n уровней каждой стороны от лучшей цены. Ответ того же вида, что данные
/// кадра частичного стакана ({lastUpdateId, bids, asks}) — разбор тот же (depth.rs Partial, levels: строка → Decimal,
/// количество 0 — уровня нет, сортировка от лучшей цены). Принимает: ответ, n. Ошибки: текст — нет bids / asks, уровень
/// не [цена, количество] строками, не число.
pub fn parse(v: &Value, n: usize) -> Result<BookPrint, String> {
    let p: Partial<'_> =
        serde::Deserialize::deserialize(v).map_err(|e| format!("ответ не разобран: {e}"))?;
    let (bids, asks) = depth::levels(&p)?;
    Ok(BookPrint::top(&bids, &asks, n))
}

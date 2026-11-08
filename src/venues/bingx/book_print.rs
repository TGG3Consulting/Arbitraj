//! Файл: venues/bingx/book_print.rs
//! Что делает: решение 213 для BingX (решения 202/213 для всех бирж, Тигран 08.11) — подтверждение тихой пары ПО
//! СОДЕРЖИМОМУ стакана (BookVersion.print, как у MEXC и Bitget): один публичный запрос GET /openApi/spot/v1/market/depth
//! (symbol, limit = venue.depth_levels — столько уровней даёт кадр <symbol>@depth<N>), из ответа — первые N уровней
//! bids/asks; после ответа — первые N уровней стакана в памяти (book.rs print — по нему считается котировка).
//! Сравнивает ядро (core/feed_print.rs). Номер не сравнивается: у кадра @depth<N> есть data.lastUpdateId, а в ответе
//! REST номера нет (bids, asks, ts) — связать нечем.
//! Решение 238: тот же запрос и разбор — снимок стакана пары для ордера (snapshot, контракт quote_rest), когда стакан
//! потока устарел: все уровни ответа.
//! Кто вызывает: mod.rs (book_version, quote_rest).
//! От чего зависит: rest.rs (public — паузы и коды BingX там), ctx.rs (символ пары, настройки), depth.rs (levels —
//! разбор уровней, как у кадра), book.rs (Books::print), gateway/feed_health.rs.
//! Настройки: venue.depth_levels [bingx] (N), venue.quote_asset [bingx] (символ пары), адрес REST и прочее — rest.rs.
//!
//! Официально (docs-v3, Spot → Market Data → Order Book, https://bingx-api.github.io/docs-v3/; исходник BingX-API/docs-v3,
//! сборка 2026-10-21): «GET /openApi/spot/v1/market/depth», symbol («Trading pair, e.g., BTC-USDT»), limit («default 20,
//! max 1000»); ответ data — bids и asks («first element price, second element quantity»), ts («Timestamp of depth»);
//! "ip-rate-limitation":"1" — «IP Rate Limit: 500 requests per 10 seconds». В примере ответа asks идут по убыванию цены
//! (как и у кадра, depth.rs) — стороны сортируются от лучшей цены здесь. Кадр Subscribe Market Depth Data: data.bids,
//! data.asks, data.lastUpdateId. Темп подтверждений задаёт ядро (venue.quiet_confirm_max_per_s,
//! venue.quiet_confirm_min_ms; совет — docs/FIRST_RUN.md).

use serde_json::Value;

use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::gateway::feed_health::{BookPrint, BookPrints, BookVersion};

use super::book::Books;
use super::ctx::Ctx;
use super::{depth, rest};

/// Путь стакана REST.
const PATH: &str = "/openApi/spot/v1/market/depth";

/// Отпечатки стакана пары для сторожа ядра. Принимает: окружение, стаканы в памяти, тикер BingX (решение 211).
/// Возвращает: BookVersion с print (id, changed_ms, server_ms — None). Ошибки: NotConfigured (venue.depth_levels,
/// venue.quote_asset, адрес REST), связь, RateLimited, коды BingX (rest.rs), BadResponse — вид ответа.
pub async fn book_version(ctx: &Ctx, books: &Books, ticker: &str) -> GatewayResult<BookVersion> {
    let n = ctx.int("venue.depth_levels")?;
    let p = vec![("symbol", ctx.symbol(ticker)?), ("limit", n.to_string())];
    let data = rest::public(ctx, PATH, p).await?;
    let rest = parse(&data, n as usize).map_err(|why| {
        ctx.tag(GatewayError::new(
            ErrorCode::BadResponse,
            None,
            format!("BingX {PATH} {ticker}: {why}"),
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

/// Решение 238: снимок стакана пары по REST для ордера — GET /openApi/spot/v1/market/depth (symbol, limit = venue.depth_levels; раздел и лимит — в шапке файла). Принимает: окружение,
/// монету (тикер площадки). Возвращает: стакан (все уровни ответа, от лучшей цены). Ошибки: как у отпечатка.
pub async fn snapshot(
    ctx: &Ctx,
    asset: &crate::core::types::Asset,
) -> GatewayResult<crate::core::types::OrderBook> {
    let n = ctx.int("venue.depth_levels")?;
    let p = vec![
        ("symbol", ctx.symbol(&asset.ticker)?),
        ("limit", n.to_string()),
    ];
    let data = rest::public(ctx, PATH, p).await?;
    let bad = |why: String| {
        ctx.tag(GatewayError::new(
            ErrorCode::BadResponse,
            None,
            format!("BingX {PATH} {}: {why}", asset.ticker),
        ))
    };
    let bids = depth::levels(&data, "bids").map_err(bad)?;
    let asks = depth::levels(&data, "asks").map_err(bad)?;
    let ts = None;
    let (venue, asset) = (ctx.venue(), asset.clone());
    Ok(crate::gateway::rest_book::book(
        venue, asset, bids, asks, ts,
    ))
}

/// Разбор data ответа: первые n уровней каждой стороны от лучшей цены (bids по убыванию, asks по возрастанию — как у
/// кадра в depth.rs). Принимает: data, n. Ошибки: текст — нет asks / bids или уровень не [цена, количество].
pub fn parse(data: &Value, n: usize) -> Result<BookPrint, String> {
    let mut asks = depth::levels(data, "asks")?;
    let mut bids = depth::levels(data, "bids")?;
    asks.sort_by_key(|a| a.price);
    bids.sort_by_key(|b| std::cmp::Reverse(b.price));
    Ok(BookPrint::top(&bids, &asks, n))
}

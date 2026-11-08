//! Файл: venues/kucoin/book_print.rs
//! Что делает: решение 213 для KuCoin (решения 202/213 для всех бирж, Тигран 08.11) — подтверждение тихой пары ПО
//! СОДЕРЖИМОМУ стакана (BookVersion.print, как у MEXC и Bitget): один публичный запрос Get Part OrderBook
//! GET /api/v1/market/orderbook/level2_{size} — size = наименьший из документированных (feed::SNAPSHOT_SIZES: 20, 100),
//! не меньший venue.depth_levels; из ответа — первые N = venue.depth_levels уровней bids/asks; после ответа — первые N
//! уровней стакана в памяти (book.rs print — по нему считается котировка). Сравнивает ядро (core/feed_print.rs).
//! Номер не сравнивается: у кадра /spotMarket/level2Depth5|50 номера нет (data — asks, bids, timestamp), sequence ответа
//! REST сравнить не с чем.
//! Решение 238: тот же запрос и разбор — снимок стакана пары для ордера (snapshot, контракт quote_rest), когда стакан
//! потока устарел: все уровни ответа level2_{size} (size — как у отпечатка, не меньше venue.depth_levels).
//! Кто вызывает: mod.rs (book_version, quote_rest).
//! От чего зависит: rest.rs (public, пул Public — пауза после 429000 соблюдается там, reply.rs), depth.rs (levels —
//! разбор уровней, как у кадра), feed.rs (SNAPSHOT_SIZES), book.rs (Books::print), gateway/feed_health.rs.
//! Настройки: venue.depth_levels [kucoin] (N), адрес REST и прочее — rest.rs.
//!
//! Официально (спецификация SDK spec/rest/api/openapi-spot-market.json, x-api-doc https://www.kucoin.com/docs-new/
//! api-3470165): «Get Part OrderBook», GET /api/v1/market/orderbook/level2_{size}, size — «Get the depth layer, optional
//! value: 20, 100», symbol; пул Public, вес 2 (x-api-rate-limit-pool, x-api-rate-limit-weight); ответ data — time,
//! sequence («Sequence number»), bids («bids, from high to low»), asks («asks, from low to high») уровнями
//! [цена, количество] строками. Кадр Level5/Level50 (spec/ws/openapi-spot-public.json): data — asks, bids, timestamp
//! («price, size»), без номера. Живые записи: kucoin_orderbook_level2_20_CAKE-USDT.json, kucoin_ws.jsonl (04.11).
//! Лимит пула Public — квота за 30 с по IP (README папки: VIP0 2000); темп подтверждений задаёт ядро
//! (venue.quiet_confirm_max_per_s, venue.quiet_confirm_min_ms; совет — docs/FIRST_RUN.md).

use serde_json::Value;

use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::gateway::feed_health::{BookPrint, BookPrints, BookVersion};

use super::book::Books;
use super::ctx::Ctx;
use super::feed::SNAPSHOT_SIZES;
use super::reply::Pool;
use super::{depth, rest};

/// Отпечатки стакана пары для сторожа ядра. Принимает: окружение, стаканы в памяти, символ KuCoin пары, тикер KuCoin
/// (решение 211). Возвращает: BookVersion с print (id, changed_ms, server_ms — None). Ошибки: NotConfigured
/// (venue.depth_levels не задана или больше наибольшего size REST), связь, RateLimited, коды KuCoin (rest.rs),
/// BadResponse — вид ответа.
pub async fn book_version(
    ctx: &Ctx,
    books: &Books,
    symbol: &str,
    ticker: &str,
) -> GatewayResult<BookVersion> {
    let n = ctx.int("venue.depth_levels")?;
    let size = size_for(n).ok_or_else(|| {
        ctx.tag(GatewayError::new(
            ErrorCode::NotConfigured,
            None,
            format!(
                "venue.depth_levels = {n}: у KuCoin стакан REST только size {SNAPSHOT_SIZES:?} — подтверждения нет"
            ),
        ))
    })?;
    let path = format!("/api/v1/market/orderbook/level2_{size}");
    let q = vec![("symbol", symbol.to_string())];
    let data = rest::public(ctx, &path, q, Pool::Public).await?;
    let rest = parse(&data, n as usize).map_err(|why| {
        ctx.tag(GatewayError::new(
            ErrorCode::BadResponse,
            None,
            format!("KuCoin {path} {ticker}: {why}"),
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

/// Решение 238: снимок стакана пары по REST для ордера — Get Part OrderBook level2_{size} (size — наименьший из 20, 100,
/// не меньший venue.depth_levels; раздел, пул и вес — в шапке файла). Принимает: окружение, символ KuCoin пары, монету
/// (тикер KuCoin). Возвращает: стакан (все уровни ответа, от лучшей цены). Ошибки: как book_version.
pub async fn snapshot(
    ctx: &Ctx,
    symbol: &str,
    asset: &crate::core::types::Asset,
) -> GatewayResult<crate::core::types::OrderBook> {
    let n = ctx.int("venue.depth_levels")?;
    let size = size_for(n).ok_or_else(|| {
        ctx.tag(GatewayError::new(
            ErrorCode::NotConfigured,
            None,
            format!(
                "venue.depth_levels = {n}: у KuCoin стакан REST только size {SNAPSHOT_SIZES:?}"
            ),
        ))
    })?;
    let path = format!("/api/v1/market/orderbook/level2_{size}");
    let q = vec![("symbol", symbol.to_string())];
    let data = rest::public(ctx, &path, q, Pool::Public).await?;
    let bad = |why: String| {
        ctx.tag(GatewayError::new(
            ErrorCode::BadResponse,
            None,
            format!("KuCoin {path} {}: {why}", asset.ticker),
        ))
    };
    let bids = depth::levels(&data, "bids").map_err(bad)?;
    let asks = depth::levels(&data, "asks").map_err(bad)?;
    let (venue, asset) = (ctx.venue(), asset.clone());
    Ok(crate::gateway::rest_book::book(
        venue, asset, bids, asks, None,
    ))
}

/// Наименьший документированный size REST, не меньший n. Принимает: N уровней. Возвращает: size; None — больше всех.
pub fn size_for(n: u64) -> Option<u64> {
    SNAPSHOT_SIZES.iter().copied().filter(|&s| s >= n).min()
}

/// Разбор data ответа: первые n уровней каждой стороны от лучшей цены (bids по убыванию, asks по возрастанию — как у
/// кадра в depth.rs; нулевое количество — уровня нет). Принимает: data, n. Ошибки: текст — нет asks / bids или
/// уровень не [цена, количество] строками.
pub fn parse(data: &Value, n: usize) -> Result<BookPrint, String> {
    let mut asks = depth::levels(data, "asks")?;
    let mut bids = depth::levels(data, "bids")?;
    asks.sort_by_key(|a| a.price);
    bids.sort_by_key(|b| std::cmp::Reverse(b.price));
    Ok(BookPrint::top(&bids, &asks, n))
}

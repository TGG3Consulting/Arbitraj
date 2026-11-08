//! Файл: venues/xt/book_version.rs
//! Что делает: решение 213 (Тигран 08.11, для всех бирж) — версия стакана пары XT.com по REST для сторожа ядра: один
//! публичный запрос GET /v4/public/depth (symbol, limit = venue.depth_levels). Способ сравнения зависит от канала
//! стакана (venue.book_stream):
//! - depth_update (локальный стакан) — ПО НОМЕРУ: id = lastUpdateId ответа. Основание — «Orderbook manage»: номер i
//!   кадров depth_update и lastUpdateId снимка REST — одна шкала («Drop any event where i is <= lastUpdateId in the
//!   snapshot», «The first processed event should have fi <= lastUpdateId+1 AND i >= lastUpdateId+1»); номер
//!   локального стакана (local_book.rs version) учёт свежести получает на каждом кадре (ws_conn.rs, frame_ver);
//! - depth (снимки N уровней) — ПО СОДЕРЖИМОМУ (решение «202/213 (по содержимому)»): у кадра depth@{symbol},{levels}
//!   поле i описано только как «updateId» (limitDepth.md), связи с lastUpdateId REST в документации нет — первые N
//!   уровней ответа и стакана в памяти (book.rs print, взят ПОСЛЕ ответа); сравнивает ядро (core/feed_print.rs).
//!
//! Решение 237: тот же запрос и разбор уровней — снимок стакана пары для ордера (snapshot, контракт quote_rest), когда
//! стакан потока устарел: все уровни ответа (limit = venue.depth_levels, тот же предел 1–500).
//! Кто вызывает: mod.rs (book_version, quote_rest).
//! От чего зависит: rest.rs (public — пауза после 429 соблюдается там), depth.rs (RestDepth, convert), ws.rs (config —
//! канал и уровни), symbols.rs (символ пары), book.rs (Books::print), gateway/feed_health.rs.
//! Настройки: venue.book_stream, venue.depth_levels [xt] (и всё, что проверяет ws.rs::config), адрес REST — rest.rs.
//!
//! Официально: Get depth data (https://doc.xt.com/#marketdepth, `_market/3depth.md`): «GET /v4/public/depth»,
//! symbol (обязателен), limit («1~500», по умолчанию 100; описание «minimum number of queries is 100» — сколько уровней
//! вернёт XT при limit меньше 100, не сказано: берутся первые N от лучшей цены), «Limit Flow Rules 10/s/ip»; ответ
//! result {timestamp, lastUpdateId («Last updated record»), bids, asks [[цена, объём]]}. Порядок уровней не оговорён —
//! сортировка, как у снимка depth (depth.rs snapshot_levels): bids по убыванию, asks по возрастанию; нулевые — уровня нет.
//! Лимит 10/s/ip — общий с очередью снимков локального стакана (snapshots.rs, venue.depth_snapshot_per_sec): темп
//! подтверждений (venue.quiet_confirm_max_per_s) плюс темп снимков — не больше 10 (совет — docs/FIRST_RUN.md).

use rust_decimal::Decimal;
use serde_json::Value;

use crate::core::types::{Asset, OrderBook, PriceLevel};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::gateway::feed_health::{BookPrint, BookPrints, BookVersion};

use super::book::Books;
use super::ctx::Ctx;
use super::depth::{RestDepth, convert};
use super::rest;
use super::symbols::Symbols;
use super::ws::{self, Stream};

/// Путь стакана REST.
const PATH: &str = "/v4/public/depth";
/// Наибольший limit — официально «1~500».
const MAX_LIMIT: u64 = 500;

/// Версия стакана пары для сторожа ядра. Принимает: окружение, справочник пар, стаканы в памяти, тикер XT (решение
/// 211). Возвращает: BookVersion — id (depth_update) или print (depth); changed_ms, server_ms — None (timestamp ответа
/// — время ответа, времени изменения книги в ответе нет). Ошибки: NotConfigured — venue.book_stream /
/// venue.depth_levels и прочие настройки соединения (ws.rs config), limit вне 1–500; BadResponse — пары нет в
/// справочнике, вид ответа; связь, RateLimited, коды XT (rest.rs).
pub async fn book_version(
    ctx: &Ctx,
    symbols: &Symbols,
    books: &Books,
    ticker: &str,
) -> GatewayResult<BookVersion> {
    let cfg = ws::config(ctx).map_err(|m| err(ctx, ErrorCode::NotConfigured, m))?;
    if cfg.levels > MAX_LIMIT {
        let m = format!(
            "venue.depth_levels = {}: limit REST у XT 1–{MAX_LIMIT}",
            cfg.levels
        );
        return Err(err(ctx, ErrorCode::NotConfigured, m));
    }
    let symbol = symbols.symbol_of(ticker).ok_or_else(|| {
        let m = format!("XT {ticker}: пары нет в справочнике — стакан по REST не запросить");
        err(ctx, ErrorCode::BadResponse, m)
    })?;
    let n = cfg.levels as usize;
    let q = vec![("symbol", symbol), ("limit", cfg.levels.to_string())];
    let v = rest::public(ctx, PATH, q).await?;
    let bad = |why: String| {
        err(
            ctx,
            ErrorCode::BadResponse,
            format!("XT {PATH} {ticker}: {why}"),
        )
    };
    match cfg.stream {
        Stream::Update => Ok(BookVersion {
            id: Some(last_update_id(&v).map_err(bad)?),
            ..Default::default()
        }),
        Stream::Depth => {
            let rest = parse(&v, n).map_err(bad)?;
            // Свой стакан — ПОСЛЕ ответа: кадр, пришедший во время запроса, ядро увидит в свежем снимке feed_health.
            let ours = books.print(ticker, n);
            Ok(BookVersion {
                print: Some(BookPrints {
                    levels: n,
                    rest,
                    ours,
                }),
                ..Default::default()
            })
        }
    }
}

/// Решение 237: снимок стакана пары по REST для ордера — GET /v4/public/depth (symbol, limit = venue.depth_levels;
/// раздел и лимит «10/s/ip» — в шапке). Принимает: окружение, справочник пар, монету (тикер XT). Возвращает: стакан
/// (все уровни ответа, от лучшей цены; время биржи — timestamp ответа). Ошибки: как book_version.
pub async fn snapshot(ctx: &Ctx, symbols: &Symbols, asset: &Asset) -> GatewayResult<OrderBook> {
    let cfg = ws::config(ctx).map_err(|m| err(ctx, ErrorCode::NotConfigured, m))?;
    if cfg.levels > MAX_LIMIT {
        let m = format!(
            "venue.depth_levels = {}: limit REST у XT 1–{MAX_LIMIT}",
            cfg.levels
        );
        return Err(err(ctx, ErrorCode::NotConfigured, m));
    }
    let ticker = &asset.ticker;
    let symbol = symbols.symbol_of(ticker).ok_or_else(|| {
        let m = format!("XT {ticker}: пары нет в справочнике — стакан по REST не запросить");
        err(ctx, ErrorCode::BadResponse, m)
    })?;
    let q = vec![("symbol", symbol), ("limit", cfg.levels.to_string())];
    let v = rest::public(ctx, PATH, q).await?;
    rest_book(ctx, &v, asset)
}

/// Решение 237: стакан из result ответа GET /v4/public/depth (уровни — depth.rs convert, время биржи — timestamp).
/// Принимает: окружение, result, монету. Ошибки: BadResponse — вид ответа, не число.
pub fn rest_book(ctx: &Ctx, v: &Value, asset: &Asset) -> GatewayResult<OrderBook> {
    let bad = |why: String| {
        let m = format!("XT {PATH} {}: {why}", asset.ticker);
        err(ctx, ErrorCode::BadResponse, m)
    };
    let d: RestDepth<'_> =
        serde::Deserialize::deserialize(v).map_err(|e| bad(format!("ответ не разобран: {e}")))?;
    let bids = convert(&d.bids, "bids").map_err(bad)?;
    let asks = convert(&d.asks, "asks").map_err(bad)?;
    let ts = crate::gateway::rest_book::ms(v.get("timestamp").and_then(Value::as_i64));
    let (venue, asset) = (ctx.venue(), asset.clone());
    Ok(crate::gateway::rest_book::book(
        venue, asset, bids, asks, ts,
    ))
}

/// lastUpdateId ответа (result). Принимает: result. Ошибка — текст причины.
pub fn last_update_id(v: &Value) -> Result<i64, String> {
    let d: RestDepth<'_> =
        serde::Deserialize::deserialize(v).map_err(|e| format!("ответ не разобран: {e}"))?;
    Ok(d.last_update_id)
}

/// Разбор ответа (result): первые n уровней каждой стороны от лучшей цены, нулевые объёмы — уровня нет.
/// Принимает: result, n. Ошибка — текст причины (вид ответа, не число).
pub fn parse(v: &Value, n: usize) -> Result<BookPrint, String> {
    let d: RestDepth<'_> =
        serde::Deserialize::deserialize(v).map_err(|e| format!("ответ не разобран: {e}"))?;
    let keep = |l: Vec<PriceLevel>| -> Vec<PriceLevel> {
        l.into_iter().filter(|l| l.qty > Decimal::ZERO).collect()
    };
    let mut bids = keep(convert(&d.bids, "bids")?);
    let mut asks = keep(convert(&d.asks, "asks")?);
    bids.sort_by_key(|l| std::cmp::Reverse(l.price));
    asks.sort_by_key(|l| l.price);
    Ok(BookPrint::top(&bids, &asks, n))
}

/// Ошибка адаптера XT с кодом. Ошибок нет.
fn err(ctx: &Ctx, code: ErrorCode, msg: String) -> GatewayError {
    ctx.tag(GatewayError::new(code, None, msg))
}

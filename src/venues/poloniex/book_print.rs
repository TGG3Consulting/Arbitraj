//! Файл: venues/poloniex/book_print.rs
//! Что делает: решение 213 для Poloniex (решения 202/213 для всех бирж, Тигран 08.11) — подтверждение тихой пары ПО
//! СОДЕРЖИМОМУ стакана (BookVersion.print, как у MEXC, OKX, KuCoin): один публичный запрос
//! GET /markets/{symbol}/orderBook (limit = venue.depth_levels — столько уровней даёт кадр канала book), из ответа —
//! первые N уровней asks/bids; после ответа — первые N уровней стакана в памяти (book.rs print — по нему считается
//! котировка). Сравнивает ядро (core/feed_print.rs).
//! Почему не по номеру: у кадра book есть id — «id of the record (SeqId)», а в ответе REST стакана номера нет вовсе
//! (поля time, scale, asks, bids, ts) — сравнивать нечего.
//! Решение 238: тот же запрос и разбор — снимок стакана пары для ордера (snapshot, контракт quote_rest), когда стакан
//! потока устарел: все уровни ответа (levels_of).
//! Кто вызывает: mod.rs (book_version, quote_rest).
//! От чего зависит: rest.rs (public — паузы и коды Poloniex там), ctx.rs (настройки), book.rs (Books::print),
//! gateway/feed_health.rs.
//! Настройки: venue.depth_levels [poloniex] (N; у канала book 5, 10 или 20 — ws.rs), адрес REST и прочее — rest.rs.
//!
//! Официально (https://api-docs.poloniex.com/spot/api/public/market-data, «Order Book»): «GET
//! /markets/{symbol}/orderBook»; scale — «controls aggregation by price», без него «no grouping/aggregation will be
//! applied» (не шлём — как у канала book); limit — «The default value of limit is 10. Valid limit values are: 5, 10,
//! 20, 50, 100, 150» (5/10/20 канала book в нём есть); ответ — time, scale, asks («list of asks»), bids («list of
//! bids»), ts. asks / bids — плоские списки строк: в примере документации ["1.91", "47.17277487"] и ["1.9", "110",
//! "1.889", "10"] — пары «цена, количество» подряд (так и разбирается; прямой фразы нет — проверить на живом ответе,
//! tests/README.md). Порядок уровней в описании ответа не оговорён — стороны сортируются от лучшей цены здесь (как у
//! кадра, depth.rs). Лимит: «Rate Limits» (https://api-docs.poloniex.com/spot/api/) — у строки Order Book ячейка
//! пустая, рыночные запросы ограничены по IP («frequency limited based on IP»), соседняя /markets/{symbol} — 200 в
//! секунду. Темп подтверждений задаёт ядро (venue.quiet_confirm_max_per_s, venue.quiet_confirm_min_ms; совет —
//! docs/FIRST_RUN.md).

use std::str::FromStr;

use rust_decimal::Decimal;
use serde_json::Value;

use crate::core::types::{Asset, OrderBook, PriceLevel};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::gateway::feed_health::{BookPrint, BookPrints, BookVersion};

use super::book::Books;
use super::ctx::Ctx;
use super::rest;

/// Отпечатки стакана пары для сторожа ядра. Принимает: окружение, стаканы в памяти, символ Poloniex, тикер (решение
/// 211). Возвращает: BookVersion с print (id, changed_ms, server_ms — None). Ошибки: NotConfigured
/// (venue.depth_levels, адрес REST), связь, RateLimited, коды Poloniex (rest.rs), BadResponse — вид ответа.
pub async fn book_version(
    ctx: &Ctx,
    books: &Books,
    symbol: &str,
    ticker: &str,
) -> GatewayResult<BookVersion> {
    let n = ctx.int("venue.depth_levels")?;
    let path = format!("/markets/{symbol}/orderBook");
    let body = rest::public(ctx, &path, vec![("limit", n.to_string())]).await?;
    let rest = parse(&body, n as usize).map_err(|why| {
        ctx.tag(GatewayError::new(
            ErrorCode::BadResponse,
            None,
            format!("Poloniex {path}: {why}"),
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

/// Разбор ответа: первые n уровней каждой стороны от лучшей цены (bids по убыванию, asks по возрастанию); уровень с
/// количеством 0 не берётся — как у кадра (depth.rs levels). Принимает: тело ответа, n. Ошибки: текст — нет asks /
/// bids, нечётная длина списка, не число.
pub fn parse(body: &Value, n: usize) -> Result<BookPrint, String> {
    let (bids, asks) = levels_of(body)?;
    Ok(BookPrint::top(&bids, &asks, n))
}

/// Все уровни ответа (bids по убыванию, asks по возрастанию; количество 0 — уровня нет). Ошибки: как parse.
pub fn levels_of(body: &Value) -> Result<(Vec<PriceLevel>, Vec<PriceLevel>), String> {
    let mut asks = flat(body, "asks")?;
    let mut bids = flat(body, "bids")?;
    asks.sort_by_key(|a| a.price);
    bids.sort_by_key(|b| std::cmp::Reverse(b.price));
    Ok((bids, asks))
}

/// Решение 238: снимок стакана пары по REST для ордера — тот же GET /markets/{symbol}/orderBook (limit =
/// venue.depth_levels). Принимает: окружение, символ Poloniex, монету (тикер Poloniex). Возвращает: стакан (все уровни;
/// время биржи — time ответа). Ошибки: как book_version.
pub async fn snapshot(ctx: &Ctx, symbol: &str, asset: &Asset) -> GatewayResult<OrderBook> {
    let n = ctx.int("venue.depth_levels")?;
    let path = format!("/markets/{symbol}/orderBook");
    let body = rest::public(ctx, &path, vec![("limit", n.to_string())]).await?;
    let (bids, asks) = levels_of(&body).map_err(|why| {
        ctx.tag(GatewayError::new(
            ErrorCode::BadResponse,
            None,
            format!("Poloniex {path}: {why}"),
        ))
    })?;
    let ts = crate::gateway::rest_book::ms(body.get("time").and_then(Value::as_i64));
    let (venue, asset) = (ctx.venue(), asset.clone());
    Ok(crate::gateway::rest_book::book(
        venue, asset, bids, asks, ts,
    ))
}

/// Плоский список строк [цена, количество, цена, количество, …] → уровни. Ошибка — текст причины.
fn flat(body: &Value, side: &str) -> Result<Vec<PriceLevel>, String> {
    let arr = body
        .get(side)
        .and_then(Value::as_array)
        .ok_or_else(|| format!("нет {side}"))?;
    if arr.len() % 2 != 0 {
        return Err(format!("{side}: нечётное число значений ({})", arr.len()));
    }
    let num = |x: &Value| {
        let s = match x {
            Value::String(s) => s.trim().to_string(),
            Value::Number(n) => n.to_string(),
            _ => String::new(),
        };
        Decimal::from_str(&s).map_err(|_| format!("{side}: {x} не число"))
    };
    let mut out = Vec::with_capacity(arr.len() / 2);
    for p in arr.chunks(2) {
        let (price, qty) = (num(&p[0])?, num(&p[1])?);
        if qty > Decimal::ZERO {
            out.push(PriceLevel { price, qty });
        }
    }
    Ok(out)
}

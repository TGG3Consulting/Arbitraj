//! Файл: venues/kraken/book_print.rs
//! Что делает: решение 213 для Kraken (решения 202/213 для всех бирж, Тигран 08.11) — подтверждение тихой пары ПО
//! СОДЕРЖИМОМУ стакана (BookVersion.print, как у OKX, KuCoin, CoinEx): один публичный запрос GET /0/public/Depth (pair
//! — имя пары REST, count = venue.depth_levels — столько уровней держит локальный стакан канала book), из ответа —
//! первые N уровней asks/bids; после ответа — первые N уровней стакана в памяти (book.rs print — по нему считается
//! котировка). Сравнивает ядро (core/feed_print.rs).
//! Почему не по номеру: у канала book номера нет (snapshot / update — symbol, bids, asks, checksum, timestamp), у ответа
//! REST тоже ([цена, объём, время] по уровням) — сравнивать нечего; checksum кадра («CRC32 checksum for the top 10
//! bids and asks») ответ REST не даёт.
//! Кто вызывает: mod.rs (book_version).
//! От чего зависит: rest.rs (public — паузы и коды Kraken там), ctx.rs (настройки), num.rs (to_dec), book.rs
//! (Books::print), gateway/feed_health.rs.
//! Настройки: venue.depth_levels [kraken] (N; у канала book 10, 25, 100, 500, 1000 — ws.rs), адрес REST — rest.rs.
//!
//! Официально (https://docs.kraken.com/api-reference/market-data/get-order-book.md, «Get Order Book»): GET
//! /public/Depth на https://api.kraken.com/0, «Returns level 2 (L2) order book»; pair — «Asset pair to get data for»;
//! count — «Maximum number of asks/bids», minimum 1, maximum 500, default 100; ответ result — объект по имени пары,
//! asks / bids — «[<price>, <volume>, <timestamp>]». Поэтому при venue.depth_levels = 1000 (канал book это разрешает)
//! сравнить N уровней нельзя — ошибка с причиной, подтверждения нет (вопрос — docs/TASKS.md). Порядок уровней в
//! документации не оговорён — стороны сортируются от лучшей цены здесь. Лимит публичных запросов в документации не
//! описан (гид по лимитам — только про счётчик ключа); темп подтверждений задаёт ядро (venue.quiet_confirm_max_per_s,
//! venue.quiet_confirm_min_ms; совет — docs/FIRST_RUN.md).

use serde_json::Value;

use crate::core::types::PriceLevel;
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::gateway::feed_health::{BookPrint, BookPrints, BookVersion};

use super::book::Books;
use super::ctx::Ctx;
use super::{num, rest};

/// Наибольший count у GET /0/public/Depth — «maximum 500».
pub const REST_COUNT_MAX: u64 = 500;

/// Отпечатки стакана пары для сторожа ядра. Принимает: окружение, стаканы в памяти, имя пары REST (ключ AssetPairs),
/// тикер (решение 211). Возвращает: BookVersion с print (id, changed_ms, server_ms — None). Ошибки: NotConfigured
/// (venue.depth_levels не задана или больше 500, адрес REST), связь, error[] Kraken (rest.rs), BadResponse — вид ответа.
pub async fn book_version(
    ctx: &Ctx,
    books: &Books,
    pair: &str,
    ticker: &str,
) -> GatewayResult<BookVersion> {
    let n = ctx.int("venue.depth_levels")?;
    if n > REST_COUNT_MAX {
        return Err(ctx.tag(GatewayError::new(
            ErrorCode::NotConfigured,
            None,
            format!(
                "venue.depth_levels = {n}: GET /0/public/Depth отдаёт не больше {REST_COUNT_MAX} уровней — \
                 сравнить стакан по содержимому нельзя"
            ),
        )));
    }
    let q = vec![rest::p("pair", pair), rest::p("count", n.to_string())];
    let result = rest::public(ctx, "Depth", q).await?;
    let rest = parse(&result, pair, n as usize).map_err(|why| {
        ctx.tag(GatewayError::new(
            ErrorCode::BadResponse,
            None,
            format!("Kraken Depth {pair}: {why}"),
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

/// Разбор result: книга пары (по имени пары, иначе единственная запись — Kraken может вернуть внутреннее имя, например
/// XXBTZUSD на XBTUSD), первые n уровней каждой стороны от лучшей цены; уровень с объёмом 0 не берётся (как у
/// локального стакана). Принимает: result, имя пары, n. Ошибки: текст — книги пары нет, нет asks / bids, уровень не
/// [цена, объём, …].
pub fn parse(result: &Value, pair: &str, n: usize) -> Result<BookPrint, String> {
    let obj = result.as_object().ok_or("result не объект")?;
    let book = match obj.get(pair) {
        Some(b) => b,
        None if obj.len() == 1 => obj.values().next().ok_or("result пустой")?,
        None => return Err(format!("в result нет книги пары (ключи: {})", obj.len())),
    };
    let mut asks = side(book, "asks")?;
    let mut bids = side(book, "bids")?;
    asks.sort_by_key(|a| a.price);
    bids.sort_by_key(|b| std::cmp::Reverse(b.price));
    Ok(BookPrint::top(&bids, &asks, n))
}

/// Уровни [цена, объём, время] → PriceLevel (объём 0 — уровня нет). Ошибка — текст причины.
fn side(book: &Value, name: &str) -> Result<Vec<PriceLevel>, String> {
    let arr = book
        .get(name)
        .and_then(Value::as_array)
        .ok_or_else(|| format!("нет {name}"))?;
    let mut out = Vec::with_capacity(arr.len());
    for l in arr {
        let v = l
            .as_array()
            .filter(|v| v.len() >= 2)
            .ok_or_else(|| format!("{name}: уровень не [цена, объём, время]"))?;
        let dec = |x: &Value| num::to_dec(x).ok_or_else(|| format!("{name}: {x} не число"));
        let (price, qty) = (dec(&v[0])?, dec(&v[1])?);
        if !qty.is_zero() {
            out.push(PriceLevel { price, qty });
        }
    }
    Ok(out)
}

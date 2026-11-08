//! Файл: venues/mexc/book_print.rs
//! Что делает: решение 213, подтверждение тихой пары MEXC ПО СОДЕРЖИМОМУ стакана (Тигран 08.11, контракт book_version
//! с BookVersion.print): один публичный запрос GET /api/v3/depth (symbol, limit = venue.depth_levels — столько уровней
//! даёт кадр частичного стакана spot@public.limit.depth.v3.api.pb@SYM@N), из ответа — первые N уровней bids/asks;
//! после ответа — первые N уровней стакана в памяти (book.rs print — по нему считается котировка). Сравнивает ядро
//! (core/feed_print.rs). Номер lastUpdateId не используется: связи с полем version частичного стакана в документации
//! нет (решение «202/213 (MEXC)»).
//! Кто вызывает: mod.rs (book_version).
//! От чего зависит: rest.rs (public — пауза после 418/429 соблюдается там, reply.rs), ctx.rs (символ пары, настройки),
//! book.rs (Books::print), gateway/feed_health.rs.
//! Настройки: venue.depth_levels [mexc] (N), venue.quote_asset [mexc] (символ пары), адрес REST и прочее — rest.rs.
//!
//! Официально: Market Data → Order Book (https://www.mexc.com/api-docs/spot-v3/market-data-endpoints/order-book):
//! «GET /api/v3/depth», «Weight(IP): 3», параметры symbol (обязателен) и limit («Return number», «default 100; max
//! 5000»); ответ — lastUpdateId («Last Update Id»), bids («Bid [Price, Quantity ]»), asks («Ask [Price, Quantity ]»),
//! пример: {"lastUpdateId":1112416,"bids":[["15.00000","49999.00000"]],"asks":[["14.0000","1.0000"]]}.
//! Уровни разбираются так же, как кадр частичного стакана (depth.rs levels): строка → Decimal, нулевое количество —
//! уровня нет; стороны сортируются от лучшей цены (покупатели по убыванию, продавцы по возрастанию). Порядок уровней
//! в ответе в документации не описан — сортировка его не меняет, если он уже от лучшей цены.
//! Лимит: вес 3 из корзины IP (README.md папки: «IP 300 веса / 10 с»); темп подтверждений задаёт ядро
//! (venue.quiet_confirm_max_per_s, venue.quiet_confirm_min_ms; совет — docs/FIRST_RUN.md).

use std::str::FromStr;

use rust_decimal::Decimal;
use serde_json::Value;

use crate::core::types::PriceLevel;
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::gateway::feed_health::{BookPrint, BookPrints, BookVersion};

use super::book::Books;
use super::ctx::Ctx;
use super::rest;

/// Путь стакана REST.
const PATH: &str = "/api/v3/depth";

/// Отпечатки стакана пары для сторожа ядра. Принимает: окружение, стаканы в памяти, тикер MEXC (решение 211).
/// Возвращает: BookVersion с print (id, changed_ms, server_ms — None). Ошибки: NotConfigured (venue.depth_levels,
/// venue.quote_asset, адрес REST), связь, RateLimited, коды MEXC (rest.rs), BadResponse — вид ответа.
pub async fn book_version(ctx: &Ctx, books: &Books, ticker: &str) -> GatewayResult<BookVersion> {
    let n = ctx.int("venue.depth_levels")?;
    let p = vec![("symbol", ctx.symbol(ticker)?), ("limit", n.to_string())];
    let v = rest::public(ctx, PATH, p).await?;
    let rest = parse(&v, n as usize).map_err(|why| {
        ctx.tag(GatewayError::new(
            ErrorCode::BadResponse,
            None,
            format!("MEXC {PATH} {ticker}: {why}"),
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

/// Разбор ответа GET /api/v3/depth: первые n уровней каждой стороны от лучшей цены. Принимает: ответ, n.
/// Ошибки: текст — нет bids / asks, уровень не [цена, количество] строками или не число.
pub fn parse(v: &Value, n: usize) -> Result<BookPrint, String> {
    let mut bids = side(v, "bids")?;
    let mut asks = side(v, "asks")?;
    bids.sort_by_key(|l| std::cmp::Reverse(l.price));
    asks.sort_by_key(|l| l.price);
    Ok(BookPrint::top(&bids, &asks, n))
}

/// Одна сторона: [[цена, количество], …] строками; нулевое количество — уровня нет. Ошибка — текст причины.
fn side(v: &Value, k: &str) -> Result<Vec<PriceLevel>, String> {
    let arr = v
        .get(k)
        .and_then(Value::as_array)
        .ok_or_else(|| format!("нет {k}"))?;
    let mut out = Vec::with_capacity(arr.len());
    for l in arr {
        let p = l
            .as_array()
            .filter(|p| p.len() >= 2)
            .ok_or_else(|| format!("{k}: уровень не [цена, количество]"))?;
        let num = |x: &Value, what: &str| {
            x.as_str()
                .and_then(|s| Decimal::from_str(s.trim()).ok())
                .ok_or_else(|| format!("{k}: {what} {x} не число"))
        };
        let (price, qty) = (num(&p[0], "цена")?, num(&p[1], "количество")?);
        if qty > Decimal::ZERO {
            out.push(PriceLevel { price, qty });
        }
    }
    Ok(out)
}

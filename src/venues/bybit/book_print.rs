//! Файл: venues/bybit/book_print.rs
//! Что делает: решение 213 для Bybit (решения 202/213 для всех бирж, Тигран 08.11) — подтверждение тихой пары: один
//! публичный запрос GET /v5/market/orderbook (category = spot, symbol, limit = venue.depth_levels — столько уровней даёт
//! кадр orderbook.{depth}.{symbol}), из ответа — первые N уровней b/a; после ответа — первые N уровней стакана в памяти
//! (book.rs print — по нему считается котировка). Сравнивает ядро (core/feed_confirm.rs, core/feed_print.rs).
//! ПО НОМЕРУ — только при venue.depth_levels = U_LEVELS: документация связывает u ответа REST с u кадров потока только
//! этой глубины; тогда BookVersion.id = u ответа, а ws_conn.rs отмечает u каждого кадра (ConnHandle::frame_ver). Иначе
//! (совет FIRST_RUN — 50 уровней) — ПО СОДЕРЖИМОМУ (BookVersion.print, как у MEXC и Bitget). seq не используется:
//! «Cross sequence … for the smaller seq, then it means the data is generated earlier» — порядок генерации данных, не
//! «книга не менялась» (на записи 04.11 seq кадров одной пары растёт скачками в сотни при u + 1).
//! Решение 238: тот же запрос и разбор — снимок стакана пары для ордера (snapshot, контракт quote_rest), когда стакан
//! потока устарел: все уровни ответа.
//! Кто вызывает: mod.rs (book_version, quote_rest).
//! От чего зависит: rest.rs (public — паузы после 403/429/10006 соблюдаются там, reply.rs), ctx.rs (символ пары,
//! настройки), book.rs (Books::print), gateway/feed_health.rs.
//! Настройки: venue.depth_levels [bybit] (N), venue.quote_asset [bybit] (символ пары), адрес REST и прочее — rest.rs.
//!
//! Официально: Get Orderbook (https://bybit-exchange.github.io/docs/v5/market/orderbook; исходник bybit-exchange/docs,
//! docs/v5/market/orderbook.mdx, 07.11.2026): «GET /v5/market/orderbook», category «spot», limit «spot: [1, 1000]»;
//! ответ — s, b («Bid, buyer. Sort by price desc», [цена, размер] строками), a («Ask, seller. Order by price asc»), ts,
//! u («Update ID, is always in sequence … For spot, corresponds to u in the 1000-level WebSocket orderbook stream»),
//! seq. Расхождение версий документации: ветка main исходника — «corresponding to u in the wss 200-level orderbook»,
//! живая страница и исходник 07.11 — 1000 (берём живую страницу). Запись tests/data/cex_samples_2026_11_04 подтверждает,
//! что u REST и u потока orderbook.50 — разные счётчики (REST 3200727, кадры orderbook.50 — 2318704…).
//! Лимит: «No more than 600 requests are allowed in any 5-second window» на IP для всех запросов к api.bybit.com /
//! api.bytick.com (Rate Limit); отдельного лимита Get Orderbook в таблице нет. Темп задаёт ядро
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
const PATH: &str = "/v5/market/orderbook";

/// Глубина потока, чей u документация связывает с u ответа REST: «For spot, corresponds to u in the 1000-level
/// WebSocket orderbook stream» (Get Orderbook).
pub const U_LEVELS: u64 = 1000;

/// Версия стакана пары для сторожа ядра. Принимает: окружение, стаканы в памяти, тикер Bybit (решение 211).
/// Возвращает: BookVersion — print всегда; id = u ответа только при venue.depth_levels = U_LEVELS.
/// Ошибки: NotConfigured (venue.depth_levels, venue.quote_asset, адрес REST), связь, RateLimited, коды Bybit (rest.rs),
/// BadResponse — вид ответа.
pub async fn book_version(ctx: &Ctx, books: &Books, ticker: &str) -> GatewayResult<BookVersion> {
    let n = ctx.int("venue.depth_levels")?;
    let p = vec![
        ("category", "spot".to_string()),
        ("symbol", ctx.symbol(ticker)?),
        ("limit", n.to_string()),
    ];
    let v = rest::public(ctx, PATH, p).await?;
    let bad = |why: String| {
        ctx.tag(GatewayError::new(
            ErrorCode::BadResponse,
            None,
            format!("Bybit {PATH} {ticker}: {why}"),
        ))
    };
    let rest = parse(&v, n as usize).map_err(bad)?;
    let id = if n == U_LEVELS {
        Some(u_of(&v).map_err(bad)?)
    } else {
        None
    };
    // Свой стакан — ПОСЛЕ ответа: кадр, пришедший во время запроса, ядро увидит в свежем снимке feed_health.
    let ours = books.print(ticker, n as usize);
    Ok(BookVersion {
        id,
        print: Some(BookPrints {
            levels: n as usize,
            rest,
            ours,
        }),
        ..Default::default()
    })
}

/// Решение 238: снимок стакана пары по REST для ордера — GET /v5/market/orderbook (category spot, symbol, limit = venue.depth_levels; раздел и лимит — в шапке). Принимает: окружение,
/// монету (тикер площадки). Возвращает: стакан (все уровни ответа, от лучшей цены). Ошибки: как у отпечатка.
pub async fn snapshot(
    ctx: &Ctx,
    asset: &crate::core::types::Asset,
) -> GatewayResult<crate::core::types::OrderBook> {
    let n = ctx.int("venue.depth_levels")?;
    let p = vec![
        ("category", "spot".to_string()),
        ("symbol", ctx.symbol(&asset.ticker)?),
        ("limit", n.to_string()),
    ];
    let v = rest::public(ctx, PATH, p).await?;
    let bad = |why: String| {
        ctx.tag(GatewayError::new(
            ErrorCode::BadResponse,
            None,
            format!("Bybit {PATH} {}: {why}", asset.ticker),
        ))
    };
    let bids = side(&v, "b").map_err(bad)?;
    let asks = side(&v, "a").map_err(bad)?;
    let ts = None;
    let (venue, asset) = (ctx.venue(), asset.clone());
    Ok(crate::gateway::rest_book::book(
        venue, asset, bids, asks, ts,
    ))
}

/// Номер u ответа (число или строка с числом). Ошибка — текст.
pub fn u_of(v: &Value) -> Result<i64, String> {
    v.get("u")
        .and_then(super::depth::int_of)
        .ok_or_else(|| "нет u".to_string())
}

/// Разбор result ответа: первые n уровней каждой стороны от лучшей цены (b по убыванию, a по возрастанию — сортировка
/// не меняет порядок, если он уже такой по документации). Принимает: result, n. Ошибки: текст — нет b / a, уровень не
/// [цена, размер] строками или не число.
pub fn parse(v: &Value, n: usize) -> Result<BookPrint, String> {
    let mut bids = side(v, "b")?;
    let mut asks = side(v, "a")?;
    bids.sort_by_key(|l| std::cmp::Reverse(l.price));
    asks.sort_by_key(|l| l.price);
    Ok(BookPrint::top(&bids, &asks, n))
}

/// Одна сторона: [[цена, размер], …] строками; нулевой размер — уровня нет (как в local_book.rs). Ошибка — текст.
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
            .ok_or_else(|| format!("{k}: уровень не [цена, размер]"))?;
        let num = |x: &Value, what: &str| {
            x.as_str()
                .and_then(|s| Decimal::from_str(s.trim()).ok())
                .ok_or_else(|| format!("{k}: {what} {x} не число"))
        };
        let (price, qty) = (num(&p[0], "цена")?, num(&p[1], "размер")?);
        if qty > Decimal::ZERO {
            out.push(PriceLevel { price, qty });
        }
    }
    Ok(out)
}

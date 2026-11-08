//! Файл: venues/coinex/book_print.rs
//! Что делает: решение 213 для CoinEx (решения 202/213 для всех бирж, Тигран 08.11) — подтверждение тихой пары ПО
//! СОДЕРЖИМОМУ стакана (BookVersion.print, как у MEXC, Bitget, KuCoin, BingX, OKX): один публичный запрос
//! GET /v2/spot/depth (market, limit = venue.depth_levels — столько уровней даёт пуш depth.update, interval "0" — как у
//! подписки, ws_conn.rs), из ответа — первые N уровней bids/asks; после ответа — первые N уровней стакана в памяти
//! (book.rs print — по нему считается котировка). Сравнивает ядро (core/feed_print.rs).
//! Почему не по номеру: номера последовательности у стакана CoinEx нет; updated_at у пуша и у ответа — «Timestamp,
//! millisecond», без слов «время изменения книги»; checksum — «signed 32-bit integer of the full depth data», какие уровни
//! он покрывает у пуша N уровней, документация однозначно не говорит (venues/coinex/README.md). Ни то, ни другое как
//! «книга не менялась» не описано — сравниваются сами уровни.
//! Решение 238: тот же запрос и разбор — снимок стакана пары для ордера (snapshot, контракт quote_rest), когда стакан
//! потока устарел: все уровни ответа.
//! Кто вызывает: mod.rs (book_version, quote_rest).
//! От чего зависит: rest.rs (public — паузы и коды CoinEx там), ctx.rs (настройки), depth.rs (levels — разбор уровней, как
//! у пуша), book.rs (Books::print), gateway/feed_health.rs.
//! Настройки: venue.depth_levels [coinex] (N: 5, 10, 20 или 50 — ws.rs), адрес REST и прочее — rest.rs.
//!
//! Официально (Get Market Depth, https://docs.coinex.com/api/v2/spot/market/http/list-market-depth): «GET /spot/depth»;
//! market, limit («One of [5, 10, 20, 50]»), interval («Merge interval», значения от "0"); ответ data — market, is_full,
//! depth {asks, bids — [цена, количество], last, updated_at, checksum}. Частоты эндпоинта на странице нет; страница
//! лимитов (https://docs.coinex.com/api/v2/rate-limit) — «IP Rate Limit … 400/s», группы /spot/depth не называет.
//! Порядок уровней в документации не оговорён — стороны сортируются от лучшей цены (depth.rs levels не берёт уровень с
//! количеством 0 — как у пуша). Темп подтверждений задаёт ядро (venue.quiet_confirm_max_per_s,
//! venue.quiet_confirm_min_ms; совет — docs/FIRST_RUN.md).

use serde_json::Value;

use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::gateway::feed_health::{BookPrint, BookPrints, BookVersion};

use super::book::Books;
use super::ctx::Ctx;
use super::{depth, rest};

/// Путь стакана REST («/v2» — в пути запроса, rest.rs).
const PATH: &str = "/v2/spot/depth";

/// Отпечатки стакана пары для сторожа ядра. Принимает: окружение, стаканы в памяти, рынок CoinEx, тикер CoinEx
/// (решение 211). Возвращает: BookVersion с print (id, changed_ms, server_ms — None). Ошибки: NotConfigured
/// (venue.depth_levels, адрес REST), связь, RateLimited, коды CoinEx (rest.rs, reply.rs), BadResponse — вид ответа.
pub async fn book_version(
    ctx: &Ctx,
    books: &Books,
    market: &str,
    ticker: &str,
) -> GatewayResult<BookVersion> {
    let n = ctx.int("venue.depth_levels")?;
    let q = vec![
        ("market", market.to_string()),
        ("limit", n.to_string()),
        ("interval", "0".to_string()),
    ];
    let data = rest::public(ctx, PATH, q).await?;
    let rest = parse(&data, n as usize).map_err(|why| {
        ctx.tag(GatewayError::new(
            ErrorCode::BadResponse,
            None,
            format!("CoinEx {PATH} {market}: {why}"),
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

/// Решение 238: снимок стакана пары по REST для ордера — GET /v2/spot/depth (market, limit = venue.depth_levels, interval "0"; раздел и лимит — в шапке). Принимает: окружение,
/// монету (тикер площадки). Возвращает: стакан (все уровни ответа, от лучшей цены). Ошибки: как у отпечатка.
pub async fn snapshot(
    ctx: &Ctx,
    market: &str,
    asset: &crate::core::types::Asset,
) -> GatewayResult<crate::core::types::OrderBook> {
    let n = ctx.int("venue.depth_levels")?;
    let q = vec![
        ("market", market.to_string()),
        ("limit", n.to_string()),
        ("interval", "0".to_string()),
    ];
    let data = rest::public(ctx, PATH, q).await?;
    let d = data.get("depth").ok_or_else(|| {
        ctx.tag(GatewayError::new(
            ErrorCode::BadResponse,
            None,
            format!("CoinEx {PATH} {}: нет data.depth", asset.ticker),
        ))
    })?;
    let bad = |why: String| {
        ctx.tag(GatewayError::new(
            ErrorCode::BadResponse,
            None,
            format!("CoinEx {PATH} {}: {why}", asset.ticker),
        ))
    };
    let bids = depth::levels(d, "bids").map_err(bad)?;
    let asks = depth::levels(d, "asks").map_err(bad)?;
    let ts = None;
    let (venue, asset) = (ctx.venue(), asset.clone());
    Ok(crate::gateway::rest_book::book(
        venue, asset, bids, asks, ts,
    ))
}

/// Разбор data ответа: data.depth — первые n уровней каждой стороны от лучшей цены (bids по убыванию, asks по
/// возрастанию; нулевое количество — уровня нет, нет стороны — пусто, как у пуша в depth.rs). Принимает: data, n.
/// Ошибки: текст — нет data.depth или уровень не [цена, количество] строками.
pub fn parse(data: &Value, n: usize) -> Result<BookPrint, String> {
    let d = data.get("depth").ok_or("нет data.depth")?;
    let mut asks = depth::levels(d, "asks")?;
    let mut bids = depth::levels(d, "bids")?;
    asks.sort_by_key(|a| a.price);
    bids.sort_by_key(|b| std::cmp::Reverse(b.price));
    Ok(BookPrint::top(&bids, &asks, n))
}

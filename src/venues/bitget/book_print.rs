//! Файл: venues/bitget/book_print.rs
//! Что делает: решение 213, подтверждение тихой пары Bitget ПО СОДЕРЖИМОМУ стакана (Тигран 08.11, контракт
//! book_version с BookVersion.print): один публичный запрос того же снимка, что котировка исполнения решения 201
//! (rest_book.rs: GET /api/v2/spot/market/orderbook, symbol, type = step0 — без объединения цен, limit =
//! venue.depth_levels — столько уровней даёт кадр books1/books5/books15), из ответа — первые N уровней bids/asks;
//! после ответа — первые N уровней стакана в памяти (book.rs print — по нему считается котировка). Сравнивает ядро
//! (core/feed_print.rs). Номер книги не используется: seq описан только у кадра WebSocket, связи с ответом REST в
//! документации нет (решение «202/213 (Bitget)»). Снимок в память НЕ кладётся (put_rest — только котировка исполнения):
//! иначе сравнение подменило бы стакан, который проверяется.
//! Кто вызывает: mod.rs (book_version).
//! От чего зависит: rest.rs (public — пауза после 429 соблюдается там), rest_book.rs (PATH), depth.rs (levels — разбор
//! уровней [цена, количество], как у кадра Depth Channel), book.rs (Books::print), gateway/feed_health.rs.
//! Настройки: venue.depth_levels [bitget] (N), адрес REST и прочее — rest.rs.
//!
//! Официально: «Get OrderBook Depth», GET /api/v2/spot/market/orderbook, «20 times/1s (IP)», параметры symbol, type
//! (step0…step5), limit (≤ 150) — Spot → Market Classic v2 (https://www.bitget.com/docs/catalog/classic-spot-market/
//! classic-spot-market; выписка — docs/notes/bitget-analysis-2026-11-05.md, раздел 5; поля data — asks / bids
//! уровнями [цена, количество] и ts — rest_book.rs). В этой работе страница документации не открылась (прокси среды
//! разработки), разбор — тот же, что у снимка решения 201 (depth.rs levels, сортировка от лучшей цены).
//! Темп подтверждений задаёт ядро (venue.quiet_confirm_max_per_s, venue.quiet_confirm_min_ms; совет —
//! docs/FIRST_RUN.md) — в пределе 20 запросов/с IP вместе с котировками исполнения.

use serde_json::Value;

use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::gateway::feed_health::{BookPrint, BookPrints, BookVersion};

use super::book::Books;
use super::ctx::Ctx;
use super::rest_book::PATH;
use super::{depth, rest};

/// Отпечатки стакана пары для сторожа ядра. Принимает: окружение, стаканы в памяти, символ Bitget пары, тикер Bitget
/// (решение 211). Возвращает: BookVersion с print (id, changed_ms, server_ms — None). Ошибки: NotConfigured
/// (venue.depth_levels, адрес REST), связь и коды Bitget (rest.rs), BadResponse — вид ответа.
pub async fn book_version(
    ctx: &Ctx,
    books: &Books,
    symbol: &str,
    ticker: &str,
) -> GatewayResult<BookVersion> {
    let n = ctx.int("venue.depth_levels")?;
    let q = vec![
        ("symbol", symbol.to_string()),
        ("type", "step0".to_string()),
        ("limit", n.to_string()),
    ];
    let data = rest::public(ctx, PATH, q).await?;
    let rest = parse(&data, n as usize).map_err(|why| {
        ctx.tag(GatewayError::new(
            ErrorCode::BadResponse,
            None,
            format!("Bitget {PATH} {ticker}: {why}"),
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

/// Разбор data ответа: первые n уровней каждой стороны от лучшей цены (bids по убыванию, asks по возрастанию; нулевое
/// количество — уровня нет, как у кадра). Принимает: data, n. Ошибки: текст — нет asks / bids или уровень не
/// [цена, количество] строками.
pub fn parse(data: &Value, n: usize) -> Result<BookPrint, String> {
    let mut asks = depth::levels(data, "asks")?;
    let mut bids = depth::levels(data, "bids")?;
    asks.sort_by_key(|a| a.price);
    bids.sort_by_key(|b| std::cmp::Reverse(b.price));
    Ok(BookPrint::top(&bids, &asks, n))
}

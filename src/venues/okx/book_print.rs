//! Файл: venues/okx/book_print.rs
//! Что делает: решение 213 для OKX (решения 202/213 для всех бирж, Тигран 08.11) — подтверждение тихой пары ПО
//! СОДЕРЖИМОМУ стакана (BookVersion.print, как у MEXC, Bitget, KuCoin, BingX): один публичный запрос
//! GET /api/v5/market/books (instId, sz = venue.depth_levels — столько уровней даёт кадр books5 / books), из ответа —
//! первые N уровней bids/asks; после ответа — первые N уровней стакана в памяти (book.rs print — по нему считается
//! котировка). Сравнивает ядро (core/feed_print.rs).
//! Почему не по номеру: seqId есть и у ответа («Sequence ID of the current message»), и у кадров books5 / books, но
//! документация связывает их только у других пар «REST + канал»: у GET / RPI order book — «Provides sequencing parity
//! with the books-rpi WebSocket channel», у SBE — снимок /books-sbe с потоком books-l2-tbt. Для GET / Order book и
//! каналов books5 / books такой фразы нет — номер не сравнивается (вопрос Тиграну и что записать — docs/TASKS.md).
//! Кто вызывает: mod.rs (book_version).
//! От чего зависит: rest.rs (public — паузы и коды OKX там), ctx.rs (настройки), depth.rs (levels — разбор уровней, как у
//! кадра), book.rs (Books::print), gateway/feed_health.rs.
//! Настройки: venue.depth_levels [okx] (N; у OKX 5 или 400 — ws.rs), адрес REST и прочее — rest.rs.
//!
//! Официально (https://www.okx.com/docs-v5/en/, «Order Book Trading → Market Data → GET / Order book»): «GET
//! /api/v5/market/books»; instId; sz — «Order book depth per side. Maximum 400, e.g. 400 bids + 400 asks», «Default
//! returns to 1 depth data»; «Rate Limit: 40 requests per 2 seconds», «Rate limit rule: IP»; «The data will be updated
//! once every 50 milliseconds»; ответ data[0] — asks, bids (уровень [цена, количество, "0" — устаревшее, число
//! ордеров]), ts («Order book generation time»), seqId. Порядок уровней в документации не оговорён — стороны
//! сортируются от лучшей цены здесь (как у кадра, depth.rs). Темп подтверждений задаёт ядро
//! (venue.quiet_confirm_max_per_s, venue.quiet_confirm_min_ms; совет — docs/FIRST_RUN.md).

use serde_json::Value;

use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::gateway::feed_health::{BookPrint, BookPrints, BookVersion};

use super::book::Books;
use super::ctx::Ctx;
use super::{depth, rest};

/// Путь стакана REST.
const PATH: &str = "/api/v5/market/books";

/// Отпечатки стакана пары для сторожа ядра. Принимает: окружение, стаканы в памяти, instId пары, тикер OKX
/// (решение 211). Возвращает: BookVersion с print (id, changed_ms, server_ms — None). Ошибки: NotConfigured
/// (venue.depth_levels, адрес REST), связь, RateLimited, коды OKX (rest.rs, reply.rs), BadResponse — вид ответа.
pub async fn book_version(
    ctx: &Ctx,
    books: &Books,
    inst: &str,
    ticker: &str,
) -> GatewayResult<BookVersion> {
    let n = ctx.int("venue.depth_levels")?;
    let q = vec![("instId", inst.to_string()), ("sz", n.to_string())];
    let data = rest::public(ctx, PATH, q).await?;
    let rest = parse(&data, n as usize).map_err(|why| {
        ctx.tag(GatewayError::new(
            ErrorCode::BadResponse,
            None,
            format!("OKX {PATH} {inst}: {why}"),
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

/// Разбор data ответа (массив из одной строки): первые n уровней каждой стороны от лучшей цены (bids по убыванию, asks
/// по возрастанию); уровень с количеством 0 не берётся — как у снимка books5 в ws_conn.rs. Принимает: data, n.
/// Ошибки: текст — data пустой, нет asks / bids или уровень не [цена, количество, …] строками.
pub fn parse(data: &Value, n: usize) -> Result<BookPrint, String> {
    let d = data
        .as_array()
        .and_then(|a| a.first())
        .ok_or("data пустой или не массив")?;
    let keep = |v: Vec<crate::core::types::PriceLevel>| -> Vec<_> {
        v.into_iter().filter(|l| !l.qty.is_zero()).collect()
    };
    let mut asks = keep(depth::levels(d, "asks")?);
    let mut bids = keep(depth::levels(d, "bids")?);
    asks.sort_by_key(|a| a.price);
    bids.sort_by_key(|b| std::cmp::Reverse(b.price));
    Ok(BookPrint::top(&bids, &asks, n))
}

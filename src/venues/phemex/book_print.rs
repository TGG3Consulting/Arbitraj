//! Файл: venues/phemex/book_print.rs
//! Что делает: решение 213 для Phemex (решения 202/213 для всех бирж, Тигран 08.11) — подтверждение тихой пары ПО
//! СОДЕРЖИМОМУ стакана (BookVersion.print, как у OKX, CoinEx): один публичный запрос GET /md/orderbook (symbol), из
//! ответа — первые N = venue.depth_levels уровней bids/asks (столько отдаёт ядру локальный стакан, ws_conn.rs) в
//! настоящих числах по масштабам пары; после ответа — первые N уровней стакана в памяти (book.rs print — по нему
//! считается котировка). Сравнивает ядро (core/feed_print.rs).
//! Почему не по номеру: sequence есть и у ответа REST («current message sequence»), и у кадра WebSocket («Latest
//! message sequence»), но документация не говорит, что это одна шкала, — номер не сравнивается (вопрос — docs/TASKS.md).
//! Кто вызывает: mod.rs (book_version).
//! От чего зависит: rest.rs (public — паузы и коды Phemex там; reply.rs отдаёт result у /md…), ctx.rs (настройки),
//! depth.rs (levels — разбор уровней, как у кадра), num.rs (unscale), book.rs (Books::print), gateway/feed_health.rs.
//! Настройки: venue.depth_levels [phemex] (N, 1…30 — ws.rs), адрес REST и прочее — rest.rs.
//!
//! Официально (https://github.com/phemex/phemex-api-docs/blob/master/Public-Spot-API-en.md, «Query Order Book»): GET
//! /md/orderbook?symbol=<symbol>; ответ {"error": null, "id": 0, "result": {"book": {"asks": [[priceEp, qty]], "bids":
//! […]}, "depth": 30, "sequence", "timestamp", "symbol", "type": "snapshot"}} — тот же вид уровней, что у кадра
//! OrderBook WebSocket (масштабированные целые). Параметра глубины у запроса нет; в примере depth 30 — как у канала
//! («depth = 30»), поэтому N ≤ 30 сравнимо. Порядок уровней не оговорён — стороны сортируются от лучшей цены здесь.
//! Лимит частоты для этого запроса не указан; общий — «5,000 requests in 5 minutes window» на IP, превышение — блок
//! IP на 5 минут (reply.rs). Темп подтверждений задаёт ядро (venue.quiet_confirm_max_per_s, venue.quiet_confirm_min_ms;
//! совет — docs/FIRST_RUN.md).

use serde_json::Value;

use crate::core::types::PriceLevel;
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::gateway::feed_health::{BookPrint, BookPrints, BookVersion};

use super::book::Books;
use super::ctx::Ctx;
use super::symbols::SymbolRules;
use super::{depth, num, rest};

/// Путь стакана REST.
const PATH: &str = "/md/orderbook";

/// Отпечатки стакана пары для сторожа ядра. Принимает: окружение, стаканы в памяти, правила пары (символ, масштабы),
/// тикер (решение 211). Возвращает: BookVersion с print (id, changed_ms, server_ms — None). Ошибки: NotConfigured
/// (venue.depth_levels, адрес REST), связь, RateLimited, коды Phemex (rest.rs, reply.rs), BadResponse — вид ответа.
pub async fn book_version(
    ctx: &Ctx,
    books: &Books,
    rules: &SymbolRules,
    ticker: &str,
) -> GatewayResult<BookVersion> {
    let n = ctx.int("venue.depth_levels")?;
    let q = vec![("symbol", rules.symbol.clone())];
    let result = rest::public(ctx, PATH, q, false).await?;
    let rest = parse(&result, n as usize, rules.price_scale, rules.base_scale).map_err(|why| {
        ctx.tag(GatewayError::new(
            ErrorCode::BadResponse,
            None,
            format!("Phemex {PATH} {}: {why}", rules.symbol),
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

/// Разбор result: первые n уровней каждой стороны от лучшей цены (bids по убыванию, asks по возрастанию) настоящими
/// числами; уровень с количеством 0 не берётся (как у локального стакана). Принимает: result, n, масштаб цены, масштаб
/// количества. Ошибки: текст — нет book, нет bids / asks, уровень не [priceEp, qty], значение вне масштаба.
pub fn parse(
    result: &Value,
    n: usize,
    price_scale: u32,
    qty_scale: u32,
) -> Result<BookPrint, String> {
    let book = result.get("book").ok_or("нет book")?;
    let side = |name: &str, best_high: bool| -> Result<Vec<PriceLevel>, String> {
        let mut raw: Vec<_> = depth::levels(book, name)?
            .into_iter()
            .filter(|&(_, q)| q != 0)
            .collect();
        raw.sort_by_key(|&(p, _)| if best_high { -p } else { p });
        raw.into_iter()
            .take(n)
            .map(|(p, q)| {
                Ok(PriceLevel {
                    price: num::unscale(i128::from(p), price_scale)
                        .ok_or_else(|| format!("{name}: цена {p} вне масштаба"))?,
                    qty: num::unscale(i128::from(q), qty_scale)
                        .ok_or_else(|| format!("{name}: количество {q} вне масштаба"))?,
                })
            })
            .collect()
    };
    let bids = side("bids", true)?;
    let asks = side("asks", false)?;
    Ok(BookPrint::top(&bids, &asks, n))
}

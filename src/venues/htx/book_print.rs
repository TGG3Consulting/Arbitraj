//! Файл: venues/htx/book_print.rs
//! Что делает: решение 213 для HTX (решения 202/213 для всех бирж, Тигран 08.11) — подтверждение тихой пары ПО
//! СОДЕРЖИМОМУ стакана (BookVersion.print, как у MEXC, OKX, Poloniex): один публичный запрос GET /market/depth (symbol,
//! depth = venue.depth_levels — столько уровней даёт снимок mbp.refresh, type = step0 — без объединения цен), из ответа —
//! первые N уровней bids/asks; после ответа — первые N уровней стакана в памяти (book.rs print — по нему считается
//! котировка). Сравнивает ядро (core/feed_print.rs).
//! Почему не по номеру: у снимка mbp.refresh есть seqNum («Sequence number of the message»), у ответа REST — version
//! («Internal data»); документация не связывает их (seqNum — у тем MBP, version — у /market/depth и темы
//! market.$symbol.depth.$type) — номер не сравнивается.
//! Кто вызывает: mod.rs (book_version).
//! От чего зависит: rest.rs (public — паузы и коды HTX там; reply.rs отдаёт tick), ctx.rs (настройки), depth.rs
//! (levels — разбор уровней, как у кадра), book.rs (Books::print), gateway/feed_health.rs.
//! Настройки: venue.depth_levels [htx] (N; у mbp.refresh 5, 10 или 20 — ws.rs), адрес REST и прочее — rest.rs.
//!
//! Официально (https://huobiapi.github.io/docs/spot/v1/en/, «Market Data → Get Market Depth»): GET /market/depth;
//! symbol; depth — «5, 10, 20», по умолчанию 20 («when type is set to "step0", the default value of "depth" is 150»
//! — поэтому depth шлётся всегда); type — step0 … step5 (step0 — без объединения, как у mbp.refresh); ответ {ch, status,
//! ts, tick: {ts, version, bids — «[price, size]», asks — «[price, size]»}}. Порядок уровней не оговорён — стороны
//! сортируются от лучшей цены здесь (как у кадра, depth.rs). Лимит частоты для этого запроса в документации не указан
//! (лимиты REST описаны по UID для приватных); темп подтверждений задаёт ядро (venue.quiet_confirm_max_per_s,
//! venue.quiet_confirm_min_ms; совет — docs/FIRST_RUN.md).

use serde_json::Value;

use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::gateway::feed_health::{BookPrint, BookPrints, BookVersion};

use super::book::Books;
use super::ctx::Ctx;
use super::{depth, rest};

/// Путь стакана REST.
const PATH: &str = "/market/depth";
/// Шаг объединения цен: step0 — «no aggregation» (как у снимка mbp.refresh).
const STEP: &str = "step0";

/// Отпечатки стакана пары для сторожа ядра. Принимает: окружение, стаканы в памяти, символ HTX, тикер (решение 211).
/// Возвращает: BookVersion с print (id, changed_ms, server_ms — None). Ошибки: NotConfigured (venue.depth_levels,
/// адрес REST), связь, RateLimited, коды HTX (rest.rs, reply.rs), BadResponse — вид ответа.
pub async fn book_version(
    ctx: &Ctx,
    books: &Books,
    symbol: &str,
    ticker: &str,
) -> GatewayResult<BookVersion> {
    let n = ctx.int("venue.depth_levels")?;
    let q = vec![
        ("symbol", symbol.to_string()),
        ("depth", n.to_string()),
        ("type", STEP.to_string()),
    ];
    let tick = rest::public(ctx, PATH, q).await?;
    let rest = parse(&tick, n as usize).map_err(|why| {
        ctx.tag(GatewayError::new(
            ErrorCode::BadResponse,
            None,
            format!("HTX {PATH} {symbol}: {why}"),
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

/// Разбор tick: первые n уровней каждой стороны от лучшей цены (bids по убыванию, asks по возрастанию); уровень с
/// количеством 0 не берётся — как у снимка (depth.rs levels). Принимает: tick, n. Ошибки: текст — tick не объект, нет
/// bids / asks, уровень не [цена, количество].
pub fn parse(tick: &Value, n: usize) -> Result<BookPrint, String> {
    if !tick.is_object() {
        return Err("нет tick".into());
    }
    for side in ["bids", "asks"] {
        if tick.get(side).is_none() {
            return Err(format!("нет {side}"));
        }
    }
    let mut asks = depth::levels(tick, "asks")?;
    let mut bids = depth::levels(tick, "bids")?;
    asks.sort_by_key(|a| a.price);
    bids.sort_by_key(|b| std::cmp::Reverse(b.price));
    Ok(BookPrint::top(&bids, &asks, n))
}

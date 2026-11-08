//! Файл: core/worst.rs
//! Что делает: худшая допустимая цена ордера: у биржи — самый глубокий уровень текущего стакана, нужный на весь объём
//! (лимитная цена: ордер замораживает qty × её); у DEX — средняя цена котировки с допуском exec.dex_slippage_pct.
//! Ту же цену берут исполнение (trade_ops.rs) и расчёт (costs_quote.rs: бюджет и пределы ордера, решение 66).
//! Решение 227: у ордера (trade_ops.rs — worst_order) худшая цена биржи считается только по стакану ядра не старше
//! venue.book_stale_ms площадки (или по подтверждённой тихой паре, решение 213) — иначе отказ «стакан устарел», как у
//! котировки адаптера. Живой случай JCT (Bitget): котировка исполнения шла по свежему снимку REST, а худшая цена — по
//! старому стакану ядра (снимок доходит до ядра асинхронно, bitget/mod.rs quote). Возраст стакана — в хронологию сделки.
//! Решение 237 (Тигран 09.11, живой случай XTER на MEXC): у ордера худшая цена биржи — по тому же стакану и той же мере
//! свежести, что котировка исполнения (exec_book.rs): снимок адаптера (кадр пары не старше venue.book_stale_ms, в том
//! числе кадр с тем же снимком, или подтверждённая тихая пара) или, если адаптер ответил «устарел», — снимок REST,
//! взятый котировкой (тот же), а без него — новый. Стакан ядра по received_at для ордера больше не используется: ядро
//! не видит кадров с тем же снимком (решение 109) — котировка проходила, ордер отказывал. venue.book_stale_ms
//! обязательна у всех бирж (не задана — ордера нет).
//! Кто вызывает: core/trade_ops.rs, core/costs_quote.rs.
//! От чего зависит: engine.rs (рынок), trade_ops.rs (вид площадки), exec_book.rs, op_error.rs, settings.
//! Настройки: exec.dex_slippage_pct (по площадке DEX); через exec_book.rs — venue.book_stale_ms, exec.rest_book_retry_ms.

use rust_decimal::Decimal;

use crate::core::types::{Asset, Side, VenueId};
use crate::gateway::contract::VenueKind;
use crate::settings::read::At;

use super::engine::{Core, rd};
use super::op_error::OpError;
use super::trade_ops::kind;

/// Худшая цена ордера и по чему посчитана (решение 227: в хронологию сделки).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Worst {
    /// Худшая допустимая цена.
    pub price: Decimal,
    /// Возраст стакана на момент расчёта, мс (биржа; от времени стакана снимка); None — DEX (от средней котировки с
    /// допуском).
    pub book_age_ms: Option<i64>,
    /// Решение 237: стакан — снимок REST (стакан потока устарел), а не стакан потока адаптера.
    pub rest: bool,
}

/// Худшая цена ОРДЕРА: у биржи — самый глубокий уровень стакана для ордера (exec_book.rs, решение 237: тот же снимок и
/// та же мера свежести, что у котировки исполнения; стакан потока устарел — снимок REST), у DEX — как worst_price.
/// Принимает: как worst_price. Возвращает: цену, возраст стакана и источник.
/// Ошибки: как worst_price (без кода); ошибки exec_book (не задана venue.book_stale_ms, REST не ответил после повтора —
/// без кода безопасного повтора: продажа — алертом, а не повторами по тому же стакану).
pub async fn worst_order(
    core: &Core,
    v: &VenueId,
    a: &Asset,
    side: Side,
    qty: Decimal,
    avg: Decimal,
) -> Result<Worst, OpError> {
    if kind(core, v)? == VenueKind::Dex {
        let price = worst_price(core, v, a, side, qty, avg)?;
        return Ok(Worst {
            price,
            book_age_ms: None,
            rest: false,
        });
    }
    let b = super::exec_book::book(core, v, a, side).await?;
    let book = b
        .snap
        .book()
        .ok_or_else(|| format!("{}: в снимке нет стакана ({})", v.0, b.snap.describe()))?;
    let age = (chrono::Utc::now() - book.received_at).num_milliseconds();
    let price = crate::gateway::frozen::deepest(book, side, qty).ok_or_else(|| {
        let src = if b.rest {
            "снимка REST"
        } else {
            "стакана"
        };
        format!(
            "{}: глубины {src} {} не хватает на {qty}",
            v.0, book.asset.ticker
        )
    })?;
    Ok(Worst {
        price,
        book_age_ms: Some(age),
        rest: b.rest,
    })
}

/// Худшая допустимая цена ордера (у биржи — лимитная цена: ордер замораживает qty × её). Ту же цену берёт расчёт
/// (costs_quote.rs) для бюджета и пределов ордера (решение 66). Ошибки: нет стакана или глубины, не задан допуск DEX.
pub fn worst_price(
    core: &Core,
    v: &VenueId,
    a: &Asset,
    side: Side,
    qty: Decimal,
    avg: Decimal,
) -> Result<Decimal, String> {
    match kind(core, v)? {
        // Решение 211: стакан площадки — по её тикеру монеты.
        VenueKind::Cex => {
            let t = super::venue_ticker::ticker_on(core, a, v);
            rd(&core.inner.market)
                .deepest_price(v, &t, side, qty)
                .ok_or_else(|| format!("{}: глубины стакана {t} не хватает на {qty}", v.0))
        }
        VenueKind::Dex => {
            let slip = core
                .inner
                .deps
                .settings
                .decimal(
                    "exec.dex_slippage_pct",
                    At {
                        venue: Some(&v.0),
                        coin: None,
                    },
                )
                .map_err(|e| e.message)?
                / Decimal::ONE_HUNDRED;
            Ok(if side == Side::Buy {
                avg * (Decimal::ONE + slip)
            } else {
                avg * (Decimal::ONE - slip)
            })
        }
    }
}

//! Файл: core/worst.rs
//! Что делает: худшая допустимая цена ордера: у ПРОДАЖИ на бирже — самый глубокий уровень текущего стакана, нужный на
//! весь объём (лимитная цена); у ПОКУПКИ на бирже при расчёте плана — самый глубокий уровень стакана расчёта (worst_price:
//! ордер замораживает qty × её), а у ордера покупки — этот же уровень ИЗ ПЛАНА (решение 262, of_plan; текущий стакан для
//! лимита покупки не берётся); у DEX — средняя цена котировки с допуском exec.dex_slippage_pct.
//! Расчёт (costs_quote.rs: бюджет и пределы ордера, решение 66) и исполнение (trade_ops.rs) берут её отсюда.
//! Решение 262 (Тигран 13.11, ТЗ «Покупка на A»: «лимитный ордер IOC … с ценой не выше худшего уровня из плана»): с
//! 3a1e429 лимит покупки биржи считался по стакану в момент ордера (worst_order) — живой случай 20261112213305466BTW:
//! худший уровень плана 0.9552, ордер ушёл с 1.48 и исполнился по 1.4797. Теперь worst_order и worst_of для покупки
//! биржи не вызываются (trade_ops.rs trade_seen берёт of_plan); продажа — как раньше, по текущему стакану.
//! Решение 227: у ордера (trade_ops.rs — worst_order) худшая цена биржи считается только по стакану ядра не старше
//! venue.book_stale_ms площадки (или по подтверждённой тихой паре, решение 213) — иначе отказ «стакан устарел», как у
//! котировки адаптера. Живой случай JCT (Bitget): котировка исполнения шла по свежему снимку REST, а худшая цена — по
//! старому стакану ядра (снимок доходит до ядра асинхронно, bitget/mod.rs quote). Возраст стакана — в хронологию сделки.
//! Решение 238 (Тигран 09.11, живой случай XTER на MEXC): у ордера худшая цена биржи — по тому же стакану и той же мере
//! свежести, что котировка исполнения (exec_book.rs): снимок адаптера (кадр пары не старше venue.book_stale_ms, в том
//! числе кадр с тем же снимком, или подтверждённая тихая пара) или, если адаптер ответил «устарел», — снимок REST,
//! взятый котировкой (тот же), а без него — новый. Стакан ядра по received_at для ордера больше не используется: ядро
//! не видит кадров с тем же снимком (решение 109) — котировка проходила, ордер отказывал. venue.book_stale_ms
//! обязательна у всех бирж (не задана — ордера нет).
//! Решение 238 (ревью, п. 3): худшая цена по уже взятому стакану (worst_of) — у первой попытки ордера продажи по
//! стакану котировки (trade_ops::trade_seen): котировка и ордер — один снимок.
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
    /// Решение 238: стакан — снимок REST (стакан потока устарел), а не стакан потока адаптера.
    pub rest: bool,
    /// Решение 262: цена — худший уровень покупки из плана (Calc::buy_worst), стакан в момент ордера не брался.
    pub plan: bool,
}

/// Решение 262: лимит ордера покупки на бирже — худший уровень стакана ПЛАНА (Calc::buy_worst: тот, по которому прошла
/// финальная котировка предпроверки и посчитан spend_max). Принимает: цену из плана. Возвращает: Worst без возраста
/// стакана (стакан в момент ордера не берётся). Ошибок нет.
pub fn of_plan(price: Decimal) -> Worst {
    Worst {
        price,
        book_age_ms: None,
        rest: false,
        plan: true,
    }
}

/// Худшая цена ОРДЕРА (кроме покупки на бирже — решение 262, of_plan): у биржи — самый глубокий уровень стакана для
/// ордера (exec_book.rs, решение 238: тот же снимок и та же мера свежести, что у котировки исполнения; стакан потока
/// устарел — снимок REST), у DEX — как worst_price.
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
            plan: false,
        });
    }
    let b = super::exec_book::book(core, v, a, side).await?;
    worst_of(v, &b, side, qty)
}

/// Решение 238 (ревью, п. 3): худшая цена ордера биржи по данному стакану для ордера (exec_book::Book) — самый глубокий
/// уровень на весь объём. Принимает: площадку, стакан, сторону, количество. Возвращает: цену, возраст стакана, источник.
/// Ошибки (без кода): в снимке нет стакана; глубины не хватает на qty.
pub fn worst_of(
    v: &VenueId,
    b: &super::exec_book::Book,
    side: Side,
    qty: Decimal,
) -> Result<Worst, OpError> {
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
        plan: false,
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

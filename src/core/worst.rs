//! Файл: core/worst.rs
//! Что делает: худшая допустимая цена ордера: у биржи — самый глубокий уровень текущего стакана, нужный на весь объём
//! (лимитная цена: ордер замораживает qty × её); у DEX — средняя цена котировки с допуском exec.dex_slippage_pct.
//! Ту же цену берут исполнение (trade_ops.rs) и расчёт (costs_quote.rs: бюджет и пределы ордера, решение 66).
//! Решение 227: у ордера (trade_ops.rs — worst_order) худшая цена биржи считается только по стакану ядра не старше
//! venue.book_stale_ms площадки (или по подтверждённой тихой паре, решение 213) — иначе отказ «стакан устарел», как у
//! котировки адаптера. Живой случай JCT (Bitget): котировка исполнения шла по свежему снимку REST, а худшая цена — по
//! старому стакану ядра (снимок доходит до ядра асинхронно, bitget/mod.rs quote). Возраст стакана — в хронологию сделки.
//! Не задана venue.book_stale_ms (у площадок, где она необязательна) — без проверки, как у их котировки.
//! Кто вызывает: core/trade_ops.rs, core/costs_quote.rs.
//! От чего зависит: engine.rs (рынок), trade_ops.rs (вид площадки), op_error.rs, settings.
//! Настройки: exec.dex_slippage_pct (по площадке DEX), venue.book_stale_ms (по площадке биржи, для ордера).

use rust_decimal::Decimal;

use crate::core::types::{Asset, Side, VenueId};
use crate::gateway::contract::VenueKind;
use crate::gateway::errors::ErrorCode;
use crate::settings::read::At;

use super::engine::{Core, rd};
use super::op_error::OpError;
use super::trade_ops::kind;

/// Худшая цена ордера и по чему посчитана (решение 227: в хронологию сделки).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Worst {
    /// Худшая допустимая цена.
    pub price: Decimal,
    /// Возраст стакана ядра на момент расчёта, мс (биржа); None — DEX (от средней котировки с допуском).
    pub book_age_ms: Option<i64>,
}

/// Худшая цена ОРДЕРА (решение 227): как worst_price, но у биржи стакан ядра не старше venue.book_stale_ms площадки
/// или подтверждён как тихий (решение 213). Принимает: как worst_price. Возвращает: цену и возраст стакана.
/// Ошибки: как worst_price (без кода); «стакан устарел» — код StaleData (ревью 08.11: ордер не ставился — продажа уходит
/// в безопасный повтор, op_error retry_safe / sell_unsent.rs, а не алертом; у Bitget снимок REST котировки доходит до
/// ядра асинхронно, venues/bitget/mod.rs, — к повтору стакан ядра уже свежий).
pub fn worst_order(
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
        });
    }
    let t = super::venue_ticker::ticker_on(core, a, v);
    let now = chrono::Utc::now().timestamp_millis();
    let (age, quiet) = {
        let m = rd(&core.inner.market);
        let b = m
            .book(v, &t)
            .ok_or_else(|| format!("{}: стакана {t} нет", v.0))?;
        (now - b.received_at.timestamp_millis(), m.quiet_until(v, &t))
    };
    let stale = core.inner.deps.settings.integer(
        "venue.book_stale_ms",
        At {
            venue: Some(&v.0),
            coin: None,
        },
    );
    if let Ok(max) = stale
        && age > max as i64
        && quiet.is_none_or(|u| now > u)
    {
        return Err(OpError {
            code: Some(ErrorCode::StaleData),
            message: format!(
                "{}: стакан {t} устарел: {age} мс (venue.book_stale_ms = {max}) — худшая цена ордера не считается",
                v.0
            ),
            sent: false,
            cancelled: false,
        });
    }
    let price = worst_price(core, v, a, side, qty, avg)?;
    Ok(Worst {
        price,
        book_age_ms: Some(age),
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

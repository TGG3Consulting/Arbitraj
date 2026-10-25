//! Файл: core/worst.rs
//! Что делает: худшая допустимая цена ордера: у биржи — самый глубокий уровень текущего стакана, нужный на весь объём
//! (лимитная цена: ордер замораживает qty × её); у DEX — средняя цена котировки с допуском exec.dex_slippage_pct.
//! Ту же цену берут исполнение (trade_ops.rs) и расчёт (costs_quote.rs: бюджет и пределы ордера, решение 66).
//! Кто вызывает: core/trade_ops.rs, core/costs_quote.rs.
//! От чего зависит: engine.rs (рынок), trade_ops.rs (вид площадки), settings.
//! Настройки: exec.dex_slippage_pct (по площадке DEX).

use rust_decimal::Decimal;

use crate::core::types::{Asset, Side, VenueId};
use crate::gateway::contract::VenueKind;
use crate::settings::read::At;

use super::engine::{Core, rd};
use super::trade_ops::kind;

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
        VenueKind::Cex => rd(&core.inner.market)
            .deepest_price(v, &a.ticker, side, qty)
            .ok_or_else(|| format!("{}: глубины стакана {} не хватает на {qty}", v.0, a.ticker)),
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

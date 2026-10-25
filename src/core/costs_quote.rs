//! Файл: core/costs_quote.rs
//! Что делает: котировка стороны сделки для полного расчёта (costs.rs) и пределы ордера биржи с видом отказа для
//! подбора объёма (решение 66):
//! - глубины не хватает или у DEX нет пути на такой объём (NoRoute: объём ≥ резерва пула) — объём слишком большой;
//! - пределы спот-пары биржи — количество (minQty / maxQty) и сумма (minNotional / maxNotional) по ХУДШЕЙ цене
//!   ордера — той же, что ставит исполнение (trade_ops::worst_price, лимитная цена) и проверяет адаптер BingX
//!   (venues/bingx/trade.rs: qty × price); соблюдаются оба предела, строже — тот, что сработал;
//! - предел 0 у максимума (maxQty, maxNotional) — «нет данных», расчёт невозможен (в записях не встречался, вопрос
//!   Тиграну, решение 66); 0 у минимума — ограничения нет;
//! - количество — вниз до шага количества пары (stepSize).
//!
//! Сбой шлюза — расчёт невозможен.
//! Кто вызывает: core/costs.rs.
//! От чего зависит: core/types, core/size.rs (Fail), deps.rs (шлюз), trade_ops.rs, gateway/events.rs (SpotPairState).
//! Настройки: через trade_ops.rs — exec.dex_slippage_pct (худшая цена DEX).

use rust_decimal::Decimal;

use crate::core::types::{Asset, Direction, Quote, Side, VenueId};
use crate::gateway::errors::{ErrorCode, GatewayError};
use crate::gateway::events::SpotPairState;

use super::engine::Core;
use super::size::Fail;

/// Котировка с проверкой глубины. Принимает: ядро, направление, сторону (true — продажа на B), количество.
/// Ошибки: TooLarge — глубины не хватает или NoRoute у площадки; Stop — прочие сбои шлюза.
pub async fn quote(core: &Core, d: &Direction, sell: bool, qty: Decimal) -> Result<Quote, Fail> {
    let (venue, side) = if sell {
        (&d.sell_on, Side::Sell)
    } else {
        (&d.buy_on, Side::Buy)
    };
    let gw = &core.inner.deps.gw;
    let v = gw.get(venue).await.map_err(|e| Fail::Stop(e.message))?;
    let q = v
        .quote(&d.asset, side, qty)
        .await
        .map_err(|e| classify(&venue.0, &e))?;
    if q.fillable_qty < qty {
        return Err(Fail::TooLarge(format!(
            "{}: глубины хватает на {} из {qty}",
            venue.0, q.fillable_qty
        )));
    }
    Ok(q)
}

/// Вид отказа котировки площадки. Принимает: площадку (для текста), ошибку шлюза. Возвращает: TooLarge — NoRoute
/// (у DEX: объём не меньше резерва пула, «расчёт обмена невозможен» — меньший объём может пройти), иначе Stop.
pub fn classify(venue: &str, e: &GatewayError) -> Fail {
    let t = format!("{venue}: {}", e.message);
    if e.code == ErrorCode::NoRoute {
        Fail::TooLarge(t)
    } else {
        Fail::Stop(t)
    }
}

/// Худшая цена ордера, как её поставит исполнение (trade_ops::worst_price). Ошибки: Stop — нет стакана или допуска.
pub fn worst(
    core: &Core,
    v: &VenueId,
    a: &Asset,
    side: Side,
    qty: Decimal,
    avg: Decimal,
) -> Result<Decimal, Fail> {
    super::trade_ops::worst_price(core, v, a, side, qty, avg).map_err(Fail::Stop)
}

/// Количество, округлённое вниз до шага пары биржи (у DEX пары нет — без изменений; шаг DEX — 10^-decimals
/// монеты — адаптер применяет сам, одинаково в котировке и в обмене). Ошибок нет.
pub fn floor_step(qty: Decimal, pair: Option<&SpotPairState>) -> Decimal {
    match pair {
        Some(p) if p.step_size > Decimal::ZERO => (qty / p.step_size).floor() * p.step_size,
        _ => qty,
    }
}

/// Пределы ордера биржи. Принимает: площадку (для текста), пару (None — DEX, пределов нет), количество, худшую цену.
/// Ошибки: TooSmall — меньше minQty или minNotional; TooLarge — больше maxQty или maxNotional; Stop — максимум 0.
pub fn order_limits(
    venue: &str,
    pair: Option<&SpotPairState>,
    qty: Decimal,
    price: Decimal,
) -> Result<(), Fail> {
    let Some(p) = pair else { return Ok(()) };
    let sum = qty * price;
    let checks = [
        ("количество", qty, p.min_qty, p.max_qty, "minQty", "maxQty"),
        (
            "сумма ордера",
            sum,
            p.min_notional,
            p.max_notional,
            "minNotional",
            "maxNotional",
        ),
    ];
    for (what, v, min, max, min_name, max_name) in checks {
        if let Some(min) = min
            && v < min
        {
            return Err(Fail::TooSmall(format!(
                "{venue}: {what} {} меньше {min_name} {min}",
                v.round_dp(8)
            )));
        }
        match max {
            Some(m) if m.is_zero() => {
                return Err(Fail::Stop(format!(
                    "{venue}: {max_name} = 0 — предел неизвестен, сделки нет"
                )));
            }
            Some(m) if v > m => {
                return Err(Fail::TooLarge(format!(
                    "{venue}: {what} {} больше {max_name} {m}",
                    v.round_dp(8)
                )));
            }
            _ => {}
        }
    }
    Ok(())
}

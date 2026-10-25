//! Файл: core/costs_quote.rs
//! Что делает: котировка стороны сделки для полного расчёта (costs.rs) с видом отказа для подбора объёма
//! (решение 66): глубины не хватает — объём слишком большой; правила спот-пары биржи — сумма ордера меньше
//! minNotional (слишком маленький) или больше maxNotional (слишком большой); количество округляется вниз до шага
//! количества пары (stepSize). Сбой шлюза — расчёт невозможен.
//! Кто вызывает: core/costs.rs.
//! От чего зависит: core/types, core/size.rs (Fail), deps.rs (шлюз), gateway/events.rs (SpotPairState).
//! Настройки: нет.

use rust_decimal::Decimal;

use crate::core::types::{Direction, Quote, Side};
use crate::gateway::events::SpotPairState;

use super::deps::Deps;
use super::size::Fail;

/// Котировка с проверкой глубины. Принимает: шлюз, направление, сторону (true — продажа на B), количество.
/// Ошибки: Stop — шлюз; TooLarge — глубины не хватает.
pub async fn quote(deps: &Deps, d: &Direction, sell: bool, qty: Decimal) -> Result<Quote, Fail> {
    let (venue, side) = if sell {
        (&d.sell_on, Side::Sell)
    } else {
        (&d.buy_on, Side::Buy)
    };
    let v = deps
        .gw
        .get(venue)
        .await
        .map_err(|e| Fail::Stop(e.message))?;
    let q = v
        .quote(&d.asset, side, qty)
        .await
        .map_err(|e| Fail::Stop(format!("{}: {}", venue.0, e.message)))?;
    if q.fillable_qty < qty {
        return Err(Fail::TooLarge(format!(
            "{}: глубины хватает на {} из {qty}",
            venue.0, q.fillable_qty
        )));
    }
    Ok(q)
}

/// Количество, округлённое вниз до шага пары биржи (у DEX пары нет — без изменений; шаг DEX — 10^-decimals
/// монеты — адаптер применяет сам, одинаково в котировке и в обмене). Ошибок нет.
pub fn floor_step(qty: Decimal, pair: Option<&SpotPairState>) -> Decimal {
    match pair {
        Some(p) if p.step_size > Decimal::ZERO => (qty / p.step_size).floor() * p.step_size,
        _ => qty,
    }
}

/// Сумма ордера в пределах пары биржи. Принимает: площадку (для текста), пару (None — DEX), сумму в USDT.
/// Ошибки: TooSmall — меньше minNotional; TooLarge — больше maxNotional.
pub fn notional(venue: &str, pair: Option<&SpotPairState>, sum: Decimal) -> Result<(), Fail> {
    let Some(p) = pair else { return Ok(()) };
    if let Some(min) = p.min_notional
        && sum < min
    {
        return Err(Fail::TooSmall(format!(
            "{venue}: сумма ордера {} меньше минимальной {min}",
            sum.round_dp(8)
        )));
    }
    if let Some(max) = p.max_notional
        && !max.is_zero()
        && sum > max
    {
        return Err(Fail::TooLarge(format!(
            "{venue}: сумма ордера {} больше максимальной {max}",
            sum.round_dp(8)
        )));
    }
    Ok(())
}

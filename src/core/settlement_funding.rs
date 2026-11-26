//! Файл: core/settlement_funding.rs.
//! Что делает: находит минимальный ордер с достаточным зачислением после вывода.
//! Кто вызывает: модель поручения и исполнитель.
//! От чего зависит: сетевой тариф, точность биржи и замороженный стакан.
//! Настройки: venue.quote_asset и сетевые имена площадок.
use super::{
    engine::Core,
    settlement::{Conversion, point_conversion},
    settlement_market::Market,
    settlement_quotes::terms,
    types::Side,
};
use rust_decimal::Decimal;
pub(super) fn funding_wire(
    net: &crate::core::types::CoinNetworkInfo,
    amount: Decimal,
) -> Result<Decimal, String> {
    if net.use_net_withdrawal_budget {
        return super::withdraw_amount::amounts(amount, net)
            .map(|v| v.wire)
            .map_err(|e| e.to_string());
    }
    let sent = super::costs_withdraw::floor_precision(amount, Some(net));
    if net.withdraw_min.is_none_or(|x| sent < x) || net.withdraw_max.is_some_and(|x| sent > x) {
        return Err("Расчётный расчётного актива вне подтверждённых пределов вывода".into());
    }
    let fee = net
        .fee_for(sent)
        .filter(|f| *f >= Decimal::ZERO)
        .ok_or("Нет комиссии вывода расчётного актива")?;
    sent.checked_sub(fee)
        .filter(|x| *x > Decimal::ZERO)
        .ok_or_else(|| "Комиссия исчерпывает сумму вывода".into())
}

/// Минимальное число шагов ордера с подтверждённым зачислением required на конверсии. Проверяются
/// фактические комиссии книги, сетевые пределы и обе точности; промежуточные оценки не становятся заявками.
pub(super) async fn fund_for(
    core: &Core,
    market: &Market,
    required: Decimal,
) -> Result<Conversion, super::size::Fail> {
    use rust_decimal::prelude::ToPrimitive;
    let (network, rules, book) = terms(core, market, Side::Buy).await?;
    if network.withdraw_min.is_none()
        || network.withdraw_fee.is_none_or(|v| v < Decimal::ZERO)
        || network.withdraw_fee_pct.is_some_and(|v| {
            v < Decimal::ZERO || (network.use_net_withdrawal_budget && !v.is_zero())
        })
    {
        return Err("Нет подтверждённых условий удержания комиссии и минимума вывода".into());
    }
    let snapshot = book
        .book()
        .ok_or("Нет книги финансирования расчётного актива")?;
    let level = snapshot
        .asks
        .first()
        .filter(|v| v.price > Decimal::ZERO)
        .ok_or("Нет предложения расчётного актива")?;
    let step = rules.step_size;
    if step <= Decimal::ZERO || required <= Decimal::ZERO {
        return Err("Неположительная потребность в расчётного актива или шаг".into());
    }
    let capacity = snapshot
        .asks
        .iter()
        .try_fold(Decimal::ZERO, |n, l| n.checked_add(l.qty))
        .ok_or("Глубина вне диапазона")?;
    let lot = |n: Decimal| {
        n.checked_div(step)
            .and_then(|v| v.floor().to_u128())
            .ok_or("Количество шагов вне диапазона")
    };
    let maximum = capacity.min(rules.max_qty.unwrap_or(capacity));
    let mut high = lot(maximum)?;
    let mut low = lot(required
        .max(rules.min_qty.unwrap_or_default())
        .max(rules.min_notional.unwrap_or_default() / level.price))?;
    let mut chosen = None;
    while low <= high {
        let mid = low + (high - low) / 2;
        let qty = quantity(mid, step).ok_or("Размер ордера вне диапазона")?;
        if qty <= Decimal::ZERO {
            low = mid + 1;
            continue;
        }
        let Some(price) = crate::gateway::frozen::deepest(snapshot, Side::Buy, qty) else {
            return Err("Книга не покрывает заявленную глубину".into());
        };
        if rules.max_notional.is_some_and(|v| qty * price > v)
            || network
                .withdraw_max
                .filter(|v| *v > Decimal::ZERO)
                .is_some_and(|v| qty > v && chosen.is_some())
        {
            if mid == 0 {
                break;
            }
            high = mid - 1;
            continue;
        }
        if rules.min_notional.is_some_and(|v| qty * price < v)
            || rules.min_qty.is_some_and(|v| qty < v)
        {
            low = mid + 1;
            continue;
        }
        let q = point_conversion(&book, &rules, market, Side::Buy, qty).await?;
        if network
            .withdraw_max
            .filter(|v| *v > Decimal::ZERO)
            .is_some_and(|v| q.net_asset > v)
        {
            if mid == 0 {
                break;
            }
            high = mid - 1;
            continue;
        }
        let wire = funding_wire(&network, q.net_asset);
        if wire.is_ok_and(|v| v >= required) {
            chosen = Some(q);
            if mid == 0 {
                break;
            }
            high = mid - 1;
        } else {
            low = mid + 1;
        }
    }
    chosen.ok_or_else(||super::size::Fail::TooLarge("Нет объёма конверсии, покрывающего финансирование в доступной глубине и лимитах вывода".into()))
}

fn quantity(lots: u128, step: Decimal) -> Option<Decimal> {
    use rust_decimal::prelude::FromPrimitive;
    Decimal::from_u128(lots)?.checked_mul(step)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lot_count_must_fit_a_decimal_before_multiplication() {
        assert_eq!(quantity(u128::MAX, Decimal::ONE), None);
        assert_eq!(
            quantity((1_u128 << 96) - 1, Decimal::ONE),
            Some(Decimal::MAX)
        );
        assert_eq!(quantity((1_u128 << 96) - 1, Decimal::TWO), None);
    }
}

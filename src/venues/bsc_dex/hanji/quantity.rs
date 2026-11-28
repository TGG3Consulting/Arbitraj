//! Подбирает минимальный вход для точного количества монеты экономической ноги.
//! Использует ту же целочисленную комиссию Hanji, что и проверка полученного остатка.
use super::{Fill, Level, RATE_SCALE, Side, Units};
use alloy_primitives::U256;

/// Защита цены для пробы в контексте общего исполнителя. В ABI допустимы шесть значащих цифр.
pub(super) fn limit_price(price: u128, slip: U256, buy_x: bool) -> Option<u128> {
    let scale = U256::from(RATE_SCALE);
    if price == 0 || slip >= scale {
        return None;
    }
    let widened = if buy_x {
        U256::from(price).checked_mul(scale + slip)? / scale
    } else {
        U256::from(price).checked_mul(scale - slip)?.div_ceil(scale)
    };
    let value = u128::try_from(widened).ok()?.max(1);
    let quantum = 10u128.pow(value.ilog10().saturating_sub(5));
    let rounded = if buy_x {
        value / quantum * quantum
    } else {
        value.div_ceil(quantum) * quantum
    };
    (rounded > 0 && rounded < 1u128 << 72).then_some(rounded)
}

impl Units {
    /// Границы уровней и последний целый лот под потолком в монете ноги (X либо Y).
    /// Значение уровня — целые единицы Y до комиссии; порядок потребления тот же, что в fill.
    pub(super) fn quantity_points(
        &self, levels: &[Level], side: Side, maximum: U256, coin_is_x: bool,
    ) -> Option<Vec<U256>> {
        let scale = U256::from(RATE_SCALE);
        let rate = U256::from(self.fee_rate);
        if self.x.is_zero() || self.y.is_zero() || rate >= scale {
            return None;
        }
        let abi_max = U256::from(u128::MAX);
        let lot_limit = if coin_is_x { (maximum / self.x).min(abi_max) } else { abi_max };
        let y_limit = if coin_is_x { abi_max } else { (maximum / self.y).min(abi_max) };
        let value_limit = match side {
            Side::BuyX => y_limit * scale / (scale + rate),
            // floor(value * (scale-rate)/scale) <= y_limit.
            Side::SellX if !coin_is_x => (((y_limit + U256::ONE) * scale).div_ceil(scale - rate) - U256::ONE).min(abi_max),
            Side::SellX => abi_max,
        };
        let mut points = Vec::new();
        let (mut lots, mut value) = (U256::ZERO, U256::ZERO);
        let mut previous = None;
        for level in levels {
            if level.price == 0 || level.price >= 1u128 << 72 || level.shares == 0
                || previous.is_some_and(|last| match side {
                    Side::BuyX => level.price < last,
                    Side::SellX => level.price > last,
                })
            {
                return None;
            }
            previous = Some(level.price);
            let price = U256::from(level.price);
            let shares = U256::from(level.shares);
            let take = shares.min(lot_limit - lots).min((value_limit - value) / price);
            if take.is_zero() {
                break;
            }
            lots += take;
            value += take * price;
            let quantity = if coin_is_x {
                lots.checked_mul(self.x)?
            } else {
                let fee = (value * rate).div_ceil(scale);
                (match side { Side::BuyX => value + fee, Side::SellX => value - fee }).checked_mul(self.y)?
            };
            if !quantity.is_zero() {
                points.push(quantity);
            }
            if take < shares {
                break;
            }
        }
        Some(points)
    }

    /// Количество указано в единицах покупаемой или продаваемой монеты. Отклоняет неполный
    /// объём и округление до другого количества, в том числе когда монета является Y рынка.
    pub fn exact_quantity(
        &self,
        levels: &[Level],
        side: Side,
        quantity: U256,
        quantity_is_output: bool,
    ) -> Option<Fill> {
        if quantity.is_zero() || self.x.is_zero() || self.y.is_zero() {
            return None;
        }
        if !quantity_is_output {
            let fill = self.fill(levels, side, quantity)?;
            return (fill.spent == quantity).then_some(fill);
        }
        let scale = U256::from(RATE_SCALE);
        let rate = U256::from(self.fee_rate);
        if rate >= scale {
            return None;
        }
        let mut left = match side {
            Side::BuyX => {
                if !(quantity % self.x).is_zero() {
                    return None;
                }
                quantity / self.x
            }
            Side::SellX => {
                if !(quantity % self.y).is_zero() {
                    return None;
                }
                // Чистая выручка равна floor(value * (scale - rate) / scale).
                (quantity / self.y)
                    .checked_mul(scale)?
                    .div_ceil(scale - rate)
            }
        };
        let mut budget = U256::ZERO;
        for level in levels {
            if left.is_zero() {
                break;
            }
            let price = U256::from(level.price);
            if price.is_zero() {
                return None;
            }
            let available = U256::from(level.shares);
            match side {
                Side::BuyX => {
                    let take = left.min(available);
                    budget = budget.checked_add(take.checked_mul(price)?)?;
                    left -= take;
                }
                Side::SellX => {
                    let take = left.div_ceil(price).min(available);
                    budget = budget.checked_add(take)?;
                    left = left.saturating_sub(take.checked_mul(price)?);
                }
            }
        }
        if !left.is_zero() {
            return None;
        }
        let budget = match side {
            Side::BuyX => budget
                .checked_add(budget.checked_mul(rate)?.div_ceil(scale))?
                .checked_mul(self.y)?,
            Side::SellX => budget.checked_mul(self.x)?,
        };
        let fill = self.fill(levels, side, budget)?;
        (fill.received == quantity && fill.unspent.is_zero()).then_some(fill)
    }
}

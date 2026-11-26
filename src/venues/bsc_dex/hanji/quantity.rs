//! Подбирает минимальный вход для точного количества монеты экономической ноги.
//! Использует ту же целочисленную комиссию Hanji, что и проверка полученного остатка.
use super::{Fill, Level, RATE_SCALE, Side, Units};
use alloy_primitives::U256;

impl Units {
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
                (quantity / self.y).checked_mul(scale)?.div_ceil(scale - rate)
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
            Side::BuyX => budget.checked_add(budget.checked_mul(rate)?.div_ceil(scale))?
                .checked_mul(self.y)?,
            Side::SellX => budget.checked_mul(self.x)?,
        };
        let fill = self.fill(levels, side, budget)?;
        (fill.received == quantity && fill.unspent.is_zero()).then_some(fill)
    }
}

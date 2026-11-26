//! Стакан Hanji содержит целые лоты X. Цена измеряется в единицах Y за лот X.
//! Комиссия считается в Y. На покупку округляется сумма затрат вверх, при продаже
//! проверяется выручка после комиссии. Остаток входа хранится отдельно.
use alloy_primitives::U256;

const RATE_SCALE: u64 = 1_000_000_000_000_000_000;

#[derive(Debug, Clone, Copy)]
pub struct Level {
    pub price: u128,
    pub shares: u128,
}

#[derive(Debug, Clone, Copy)]
pub struct Units {
    pub x: U256,
    pub y: U256,
    pub fee_rate: u128,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    BuyX,
    SellX,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fill {
    pub spent: U256,
    pub received: U256,
    pub unspent: U256,
    pub fee_y: U256,
    pub last_price: u128,
}

impl Units {
    /// Расчёт по видимым уровням, с суммой комиссии вверх до минимальной единицы контракта.
    /// Вернёт None, если объёма нет, утрачен порядок цен или значение выходит за предел ABI.
    pub fn fill(&self, levels: &[Level], side: Side, budget: U256) -> Option<Fill> {
        let scale = U256::from(RATE_SCALE);
        let rate = U256::from(self.fee_rate);
        if self.x.is_zero() || self.y.is_zero() || rate >= scale || budget.is_zero() {
            return None;
        }
        let max_value = U256::from(u128::MAX);
        let raw_budget = budget / if side == Side::BuyX { self.y } else { self.x };
        if raw_budget > max_value {
            return None;
        }
        // For an integer budget B, v + ceil(v*r/S) <= B iff v <= floor(B*S/(S+r)).
        let capacity = match side {
            Side::BuyX => raw_budget.checked_mul(scale)? / (scale + rate),
            Side::SellX => raw_budget,
        };
        let mut lots = U256::ZERO;
        let mut value = U256::ZERO;
        let mut previous = None;
        let mut last_price = 0;
        for &Level { price, shares } in levels {
            if price == 0
                || price >= (1u128 << 72)
                || shares == 0
                || previous.is_some_and(|last| match side {
                    Side::BuyX => price < last,
                    Side::SellX => price > last,
                })
            {
                return None;
            }
            previous = Some(price);
            let p = U256::from(price);
            let available = match side {
                Side::BuyX => capacity.checked_sub(value)? / p,
                Side::SellX => capacity.checked_sub(lots)?,
            };
            let take = U256::from(shares).min(available);
            if !take.is_zero() {
                last_price = price;
                lots = lots.checked_add(take)?;
                value = value.checked_add(take.checked_mul(p)?)?;
            }
        }
        if lots.is_zero() || lots > max_value || value > max_value {
            return None;
        }
        let fee = value.checked_mul(rate)?.div_ceil(scale);
        let token_x = lots.checked_mul(self.x)?;
        let (spent, received) = match side {
            Side::BuyX => ((value + fee).checked_mul(self.y)?, token_x),
            Side::SellX => (token_x, value.checked_sub(fee)?.checked_mul(self.y)?),
        };
        if received.is_zero() {
            return None;
        }
        Some(Fill {
            spent,
            received,
            unspent: budget.checked_sub(spent)?,
            fee_y: fee.checked_mul(self.y)?,
            last_price,
        })
    }
}

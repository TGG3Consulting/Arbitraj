//! Единицы ордера каждой площадки на один токен контракта. Храним оба множителя:
//! преждевременно вычисленное 1/3 превращает три биржевых единицы в неполный токен.

use alloy_primitives::aliases::U512;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct UnitRatio {
    pub buy_per_raw: Decimal,
    pub sell_per_raw: Decimal,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_decimals: Option<u8>,
}

impl Default for UnitRatio {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl UnitRatio {
    pub const IDENTITY: Self = Self {
        buy_per_raw: Decimal::ONE,
        sell_per_raw: Decimal::ONE,
        raw_decimals: None,
    };

    pub fn is_identity(self) -> bool {
        self.buy_per_raw == self.sell_per_raw
    }

    pub fn to_sell(self, buy: Decimal) -> Result<Decimal, String> {
        Self::convert(buy, self.buy_per_raw, self.sell_per_raw)
    }

    pub fn to_buy(self, sell: Decimal) -> Result<Decimal, String> {
        Self::convert(sell, self.sell_per_raw, self.buy_per_raw)
    }

    /// Сначала вычисляем точное отношение целых мантисс; дробный хвост, который Decimal
    /// не может хранить, отбрасываем один раз в конце. Округление по шагу выполняет площадка.
    fn convert(x: Decimal, from: Decimal, to: Decimal) -> Result<Decimal, String> {
        if from <= Decimal::ZERO || to <= Decimal::ZERO {
            return Err("единица площадки должна быть положительной".into());
        }
        if from == to || x.is_zero() {
            return Ok(x);
        }
        let ten = U512::from(10);
        let pow = |s: u32| ten.pow(U512::from(s));
        let mant = |v: Decimal| U512::from(v.mantissa().unsigned_abs());
        let mut scale = Decimal::MAX_SCALE;
        let numerator = mant(x) * mant(to) * pow(from.scale() + scale);
        let divisor = mant(from) * pow(x.scale() + to.scale());
        let mut out = numerator / divisor;
        let max = U512::from(u128::MAX >> 32);
        while out > max && scale > 0 {
            out /= ten;
            scale -= 1;
        }
        if out > max {
            return Err("сумма после перевода единиц не помещается в Decimal".into());
        }
        let amount = out.to::<u128>() as i128;
        Ok(Decimal::from_i128_with_scale(
            if x.is_sign_negative() {
                -amount
            } else {
                amount
            },
            scale,
        )
        .normalize())
    }
}

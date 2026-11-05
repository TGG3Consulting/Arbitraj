//! Файл: venues/binance/fee_rates.rs
//! Что делает: ставки комиссии одной пары Binance (стандартная, налоговая, специальная; taker и buyer/seller) и
//! скидка своей монетой (BNB) — разбор ответа и выбор ставки для расчёта по варианту venue.fee_deduction (решение 158).
//! Кто вызывает: fees.rs (разбор), book.rs (plan — в котировке), тесты.
//! От чего зависит: num.rs.
//! Настройки: нет (вариант передаёт book.rs).
//!
//! Официально:
//! - GET /api/v3/account/commission?symbol= (вес 20, rest-api «Query Commission Rates»): standardCommission,
//!   taxCommission, specialCommission — по {maker, taker, buyer, seller}; discount {enabledForAccount, enabledForSymbol,
//!   discountAsset, discount}.
//! - faqs/commission_faq: комиссия каждого вида = сумма × (ставка роли + ставка стороны): у тейкера-покупателя
//!   taker + buyer, у продавца taker + seller; покупка — с полученного количества, продажа — с полученных USDT.
//!   «Standard commission (Discounted and in BNB) = (Standard commission × BNB exchange rate) × discount» — discount —
//!   множитель; скидка «does not apply to tax commissions or special commissions»; при нехватке BNB «the full
//!   commission will be taken out of your received amount». Значение множителя в примерах разное (0.75 и 0.25) —
//!   проверить на живом ответе.

use rust_decimal::Decimal;
use serde_json::Value;

use crate::core::types::Side;
use crate::gateway::errors::GatewayResult;

use super::num;

/// Ставки одного вида комиссии (доли).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Kind {
    pub taker: Decimal,
    pub buyer: Decimal,
    pub seller: Decimal,
}

impl Kind {
    /// Ставка тейкера по стороне: taker + buyer (покупка) или taker + seller (продажа). Ошибок нет.
    pub fn of(&self, side: Side) -> Decimal {
        self.taker
            + match side {
                Side::Buy => self.buyer,
                Side::Sell => self.seller,
            }
    }

    /// Разбор {maker, taker, buyer, seller}; нет объекта — нули. Ошибка BadResponse — поле не число.
    fn parse(v: Option<&Value>) -> GatewayResult<Self> {
        let Some(v) = v else {
            return Ok(Self::default());
        };
        let f = |n: &str| num::dec_opt(v, n).map(|d| d.unwrap_or_default());
        Ok(Self {
            taker: f("taker")?,
            buyer: f("buyer")?,
            seller: f("seller")?,
        })
    }
}

/// Ставки пары и скидка аккаунта.
#[derive(Debug, Clone, PartialEq)]
pub struct Rates {
    pub standard: Kind,
    pub tax: Kind,
    pub special: Kind,
    /// Скидка включена на аккаунте и на паре (enabledForAccount и enabledForSymbol).
    pub discount_on: bool,
    /// Монета скидки (discountAsset, например BNB) — из ответа, не настройка.
    pub discount_asset: String,
    /// Множитель стандартной комиссии при оплате монетой скидки (discount).
    pub discount: Decimal,
}

/// Варианты venue.fee_deduction: обычная ставка в монете сделки или со скидкой в монете скидки (BNB).
pub const FEE_COIN: &str = "coin";
pub const FEE_BNB: &str = "bnb";

impl Rates {
    /// Что делает: разбор ответа account/commission. Ошибка BadResponse — нет standardCommission или поле не число.
    pub fn parse(v: &Value) -> GatewayResult<Self> {
        if v.get("standardCommission").is_none() {
            return Err(super::errors::bad(
                "account/commission без standardCommission",
                &v.to_string(),
            ));
        }
        let d = v.get("discount");
        let flag = |n: &str| d.and_then(|d| d.get(n)).and_then(Value::as_bool) == Some(true);
        Ok(Self {
            standard: Kind::parse(v.get("standardCommission"))?,
            tax: Kind::parse(v.get("taxCommission"))?,
            special: Kind::parse(v.get("specialCommission"))?,
            discount_on: flag("enabledForAccount") && flag("enabledForSymbol"),
            discount_asset: d
                .and_then(|d| num::text_opt(d, "discountAsset"))
                .unwrap_or_default(),
            discount: match d {
                Some(d) => num::dec_opt(d, "discount")?.unwrap_or_default(),
                None => Decimal::ZERO,
            },
        })
    }

    /// Полная ставка тейкера по стороне (все три вида, без скидки) — худший случай. Ошибок нет.
    pub fn full(&self, side: Side) -> Decimal {
        self.standard.of(side) + self.tax.of(side) + self.special.of(side)
    }

    /// Что делает: ставка для расчёта по варианту (решение 158). coin — полная ставка в монете сделки, даже если скидка
    /// включена (со скидкой комиссия только меньше и уходит в BNB). bnb — стандартная × discount + налог + специальная,
    /// в монете скидки, только если скидка включена на аккаунте и паре и монеты скидки хватает (discount_ok); иначе —
    /// полная ставка в монете сделки (Binance сам так берёт при нехватке BNB).
    /// Принимает: вариант, сторону, хватает ли монеты скидки. Возвращает: (ставка, в монете скидки ли, пояснение).
    /// Ошибка — текст: вариант неизвестен.
    pub fn plan(
        &self,
        mode: &str,
        side: Side,
        discount_ok: bool,
    ) -> Result<(Decimal, bool, Option<String>), String> {
        let full = self.full(side);
        match mode {
            FEE_COIN if self.discount_on => Ok((
                full,
                false,
                Some("скидка включена — расчёт по полной ставке (худший случай)".into()),
            )),
            FEE_COIN => Ok((full, false, None)),
            FEE_BNB if self.discount_on && self.discount > Decimal::ZERO && discount_ok => Ok((
                self.standard.of(side) * self.discount + self.tax.of(side) + self.special.of(side),
                true,
                None,
            )),
            FEE_BNB => Ok((
                full,
                false,
                Some(format!(
                    "скидка {} недоступна (включена: {}, discount {}, монеты хватает: {discount_ok}) — полная ставка",
                    self.discount_asset, self.discount_on, self.discount
                )),
            )),
            other => Err(format!(
                "venue.fee_deduction = {other}: у Binance варианты {FEE_COIN} (в монете сделки) или {FEE_BNB} (со скидкой)"
            )),
        }
    }
}

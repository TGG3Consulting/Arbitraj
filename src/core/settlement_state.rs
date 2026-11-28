//! Файл: core/settlement_state.rs.
//! Что делает: хранит поручение, подтверждения и остаток в валюте каждой стороны.
//! Кто вызывает: учёт маршрута и API.
//! От чего зависит: результаты конверсии и перевода.
//! Настройки: снимок рынка сохраняется вместе со сделкой.
use super::{
    settlement_cash::CashFill,
    settlement_transfer::Movement,
    types::{DexAccountId, Direction, VenueId},
};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
/// Вход для выдачи средств на конверсии. Порог потребности в расчётного актива относится к полному объёму покупки токена.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Leg {
    Fund {
        #[serde(alias = "budget_usdt")]
        quote_budget: Decimal,
        #[serde(alias = "buy_usdc")]
        buy_amount: Decimal,
        maximum_price: Decimal,
        #[serde(alias = "required_wallet_usdc")]
        required_wallet_amount: Decimal,
        target_qty: Decimal,
        sell_qty: Decimal,
        minimum_target_bid: Decimal,
    },
    Return {
        #[serde(alias = "available_usdc")]
        available_amount: Decimal,
        minimum_price: Decimal,
    },
}

/// Начальное поручение сохраняется до любого движения денег, включая исходную биржу и привязанный кошелёк.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Plan {
    pub deal: String,
    pub direction: Direction,
    pub cex: VenueId,
    pub wallet: DexAccountId,
    pub leg: Leg,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Path {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub market: Option<super::settlement_market::Market>,
    pub plan: Plan,
    pub conversion: Option<CashFill>,
    pub movement: Option<Movement>,
    /// Отметка перевода зачисленного на торговый счёт перед конверсией возврата (settlement_to_trading.rs; решение
    /// Тиграна 28.11.2026). Нет — шаг не начинался; старые записи без поля читаются как раньше.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to_trading: Option<serde_json::Value>,
}
impl Path {
    /// Разрешает восстановление только сохранённой сделки в том же рынке. Старое поручение
    /// без снимка остаётся в локальном архиве до ручной сверки.
    pub fn permits(&self, p: &Plan, m: &super::settlement_market::Market) -> bool {
        self.plan == *p && self.market.as_ref().is_some_and(|old| old.identical(m))
    }

    pub(super) fn steps_confirmed(&self) -> bool {
        self.conversion.is_some() && self.movement.is_some()
    }
}
/// Сверка фактически достигнутого денежного результата. Остаток расчётного актива после частичной продажи
/// остаётся на бирже. Недостаточный кредит на кошельке не разрешает первоначальную покупку.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub enum Progress {
    Waiting,
    Funded {
        debited_quote: Decimal,
        credited_asset: Decimal,
        unspent_quote: Decimal,
    },
    InsufficientFunding {
        debited_quote: Decimal,
        held_wallet_amount: Decimal,
    },
    Returned {
        credited_quote: Decimal,
        held_cex_amount: Decimal,
    },
}
impl Path {
    pub fn progress(&self) -> Progress {
        let (Some(c), Some(t)) = (&self.conversion, &self.movement) else {
            return Progress::Waiting;
        };
        match self.plan.leg {
            Leg::Fund {
                required_wallet_amount,
                ..
            } if t.credit.amount < required_wallet_amount => Progress::InsufficientFunding {
                debited_quote: c.debit,
                held_wallet_amount: t.credit.amount,
            },
            Leg::Fund { .. } => Progress::Funded {
                debited_quote: c.debit,
                credited_asset: t.credit.amount,
                unspent_quote: c.unspent,
            },
            Leg::Return { .. } => Progress::Returned {
                credited_quote: c.credit,
                held_cex_amount: c.unspent,
            },
        }
    }
}

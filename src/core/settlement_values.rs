//! Файл: core/settlement_values.rs.
//! Что делает: описывает денежный результат модели и границы количества.
//! Кто вызывает: оценка рынка и API.
//! От чего зависит: котировки, валюта учёта и шаги перевода.
//! Настройки: получает значения из Market.
use super::{
    settlement::Conversion,
    settlement_state::Leg,
    types::{DexAccountId, Direction, Quote, VenueId},
};
use rust_decimal::Decimal;
use serde::Serialize;
/// Стоимость/прибыль моделируются в валюты учёта. Оценки расчётного актива, лимиты ордеров и объёмы токена явно разделены.
#[derive(Serialize)]
pub struct Model {
    pub direction: Direction,
    pub source: VenueId,
    pub wallet: DexAccountId,
    pub buy: Quote,
    pub sell: Quote,
    pub buy_limit: Decimal,
    pub sell_limit: Decimal,
    pub sell_qty: Decimal,
    pub withdrawal_qty: Decimal,
    pub credited_target: Decimal,
    pub conversion: Conversion,
    pub leg: Leg,
    pub invested_quote: Decimal,
    pub capital_quote: Decimal,
    pub proceeds_quote: Decimal,
    pub other_costs_quote: Decimal,
    pub reserve_quote: Decimal,
    pub profit_quote: Decimal,
}
pub(super) fn mul(a: Decimal, b: Decimal) -> Result<Decimal, String> {
    a.checked_mul(b)
        .ok_or_else(|| "Сумма вне диапазона Decimal".into())
}
pub(super) fn add(a: Decimal, b: Decimal) -> Result<Decimal, String> {
    a.checked_add(b)
        .ok_or_else(|| "Сумма вне диапазона Decimal".into())
}

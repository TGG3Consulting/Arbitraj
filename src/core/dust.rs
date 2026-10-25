//! Файл: core/dust.rs
//! Что делает: пыль после продажи (решение 66): остаток меньше шага количества пары биржи, меньше minQty или по цене
//! продажи меньше minNotional — такой ордер биржа не примет, значит это не «застрявшая монета», а пыль: сделка
//! закрывается, остаток пишется в сделку отдельной строкой (Deal.dust) и остаётся на балансе площадки.
//! У DEX правил пары нет — остаток пылью не считается (продаётся как обычно).
//! Кто вызывает: core/sell.rs.
//! От чего зависит: engine.rs, market_read.rs (правила пары из SpotPairs).
//! Настройки: нет (пределы — из справочника пар биржи).

use rust_decimal::Decimal;

use crate::core::types::VenueId;
use crate::gateway::events::SpotPairState;

use super::engine::{Core, rd};

/// Пыль ли остаток. Принимает: ядро, площадку, тикер, остаток, цену продажи. Возвращает: true — продать нельзя.
/// Ошибок нет: правил пары нет — не пыль.
pub fn is_dust(core: &Core, v: &VenueId, ticker: &str, qty: Decimal, price: Decimal) -> bool {
    let m = rd(&core.inner.market);
    m.pair(v, ticker)
        .is_some_and(|p| below_order(p, qty, price))
}

/// Остаток не составляет ордера пары: меньше шага, minQty или minNotional по цене. Ошибок нет.
pub fn below_order(p: &SpotPairState, qty: Decimal, price: Decimal) -> bool {
    qty < p.step_size
        || p.min_qty.is_some_and(|x| qty < x)
        || p.min_notional.is_some_and(|x| qty * price < x)
}

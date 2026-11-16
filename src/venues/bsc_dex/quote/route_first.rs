//! Файл: venues/bsc_dex/quote/route_first.rs
//! Что делает: решение 336 (задача t157) — обмен хопа маршрута, который route_calc.rs считает первым (покупка — последний
//! хоп «с конца», продажа — первый хоп), отдельно: тот же расчёт, что первая итерация route_calc.rs buy / sell. Он
//! зависит только от этого хопа, объёма и налога монеты — путь через промежуточную монету (quote/via_pools.rs) считает
//! его один раз на пул монеты, а не на каждую пару пулов, и передаёт в route_calc.rs eval_from.
//! Кто вызывает: quote/via.rs (first_leg).
//! От чего зависит: quote/route_calc.rs (Leg, First, buy_step, sell_step, bad), num.rs.
//! Настройки: нет своих; через pool_quote — dex.v2_fee_pct [вид пула V2].

use rust_decimal::Decimal;

use crate::core::types::Side;
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::num;
use super::route_calc::{First, Leg, bad, buy_step, sell_step};

/// Что делает: только обмен хопа, который считается первым (тот же расчёт, что первая итерация buy / sell —
/// route_calc.rs buy_step / sell_step, с долей крючка на его входе) — он зависит лишь от этого хопа, объёма и налога:
/// путь через промежуточную считает его один раз на пул монеты, а не на каждую пару пулов (quote/via_pools.rs).
/// Принимает: как eval. Возвращает: (влияние крючка, обмен). Ошибки: как первая итерация eval.
pub fn first(
    ctx: &Ctx,
    legs: &[Leg],
    side: Side,
    qty: Decimal,
    tax_keep: Decimal,
) -> GatewayResult<First> {
    let Some(leg) = (match side {
        Side::Buy => legs.last(),
        Side::Sell => legs.first(),
    }) else {
        return Err(ctx.err(ErrorCode::NoRoute, "маршрут без пулов"));
    };
    match side {
        Side::Buy => buy_step(ctx, leg, qty, tax_keep),
        Side::Sell => {
            let amount = num::to_units(qty * tax_keep, leg.dec_in).ok_or_else(|| bad(ctx))?;
            sell_step(ctx, leg, amount)
        }
    }
}

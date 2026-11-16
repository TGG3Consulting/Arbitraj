//! Файл: venues/bsc_dex/quote/route_first.rs
//! Что делает: решение 336 (задача t157) — обмен хопа маршрута, который route_calc.rs считает первым (покупка — последний
//! хоп «с конца», продажа — первый хоп), отдельно: тот же расчёт, что первая итерация route_calc.rs buy / sell. Он
//! зависит только от этого хопа, объёма и налога монеты — путь через промежуточную монету (quote/via_pools.rs) считает
//! его один раз на пул монеты, а не на каждую пару пулов, и передаёт в route_calc.rs eval_from.
//! Кто вызывает: quote/via.rs (first_leg).
//! От чего зависит: quote/route_calc.rs (Leg, buy_keep, bad), quote/pool_quote.rs, quote/direct.rs (ceil_units), num.rs.
//! Настройки: нет своих; через pool_quote — dex.v2_fee_pct [вид пула V2].

use rust_decimal::Decimal;

use crate::core::types::Side;
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::math::swap_cl::Swapped;
use super::super::num;
use super::direct::ceil_units;
use super::pool_quote;
use super::route_calc::{Leg, bad, buy_keep};

/// Что делает: только обмен хопа, который считается первым (тот же расчёт, что первая итерация buy / sell) — он
/// зависит лишь от этого хопа, объёма и налога: путь через промежуточную считает его один раз на пул монеты, а не на
/// каждую пару пулов (quote/via_pools.rs). Принимает: как eval. Возвращает: обмен. Ошибки: как первая итерация eval.
pub fn first(
    ctx: &Ctx,
    legs: &[Leg],
    side: Side,
    qty: Decimal,
    tax_keep: Decimal,
) -> GatewayResult<Swapped> {
    let Some(leg) = (match side {
        Side::Buy => legs.last(),
        Side::Sell => legs.first(),
    }) else {
        return Err(ctx.err(ErrorCode::NoRoute, "маршрут без пулов"));
    };
    match side {
        Side::Buy => {
            let k = buy_keep(true, leg, tax_keep);
            if k <= Decimal::ZERO {
                return Err(ctx.err(ErrorCode::NoRoute, "налог покупки и крючки забирают всё"));
            }
            let pool_out = ceil_units(qty / k, leg.dec_out).ok_or_else(|| bad(ctx))?;
            pool_quote::swap(ctx, &leg.entry, leg.token_in, false, pool_out)
        }
        Side::Sell => {
            let amount = num::to_units(qty * tax_keep, leg.dec_in).ok_or_else(|| bad(ctx))?;
            pool_quote::swap(ctx, &leg.entry, leg.token_in, true, amount)
        }
    }
}

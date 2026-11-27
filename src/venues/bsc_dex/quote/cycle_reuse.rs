//! Пятая/последняя нога через тот же конвертор использует состояние после первого платежа.
use super::super::{
    ctx::Ctx,
    math::swap_cl::{self, ClState, Version},
    num,
    pools::{families::Proto, key::PoolKey, registry::Entry},
};
use super::{pool_after, pool_quote, route_calc::Leg};
use crate::{
    core::types::{PoolKind, PoolState},
    gateway::errors::{ErrorCode, GatewayResult},
};
use rust_decimal::Decimal;

fn fixed_v4(e: &Entry) -> bool {
    e.family.proto == Proto::UniswapV4
        && e.ticks.is_some()
        && matches!(&e.key,Some(PoolKey::Uniswap(k)) if k.hooks.is_zero() && k.fee<1_000_000)
        && matches!(&e.pool.state,PoolState::Cl{lp_fee,..} if matches!(&e.key,Some(PoolKey::Uniswap(k)) if k.fee==*lp_fee))
}
pub(super) fn supported(e: &Entry) -> bool {
    fixed_v4(e)
        || (e.family.variant == super::super::pools::family_variant::Variant::Base
            && matches!(e.family.proto, Proto::UniswapV3 | Proto::PancakeV3)
            && e.pool.kind == PoolKind::V3
            && pool_after::appliable(e))
}
/// Физическое состояние первой и последней ног совпадает только у расчёта через общий конвертор.
pub(super) fn repeated(legs: &[Leg]) -> bool {
    legs.len() >= 4
        && legs
            .first()
            .zip(legs.last())
            .is_some_and(|(a, b)| a.entry.liquidity_site() == b.entry.liquidity_site())
}
pub(super) fn after(ctx: &Ctx, legs: &[Leg], input: Decimal) -> GatewayResult<Vec<Leg>> {
    let first = &legs[0];
    let last = &legs[legs.len() - 1];
    let e = &first.entry;
    let bad = || {
        ctx.err(
            ErrorCode::NoRoute,
            "Не подтверждено состояние общего конвертора после первой ноги",
        )
    };
    if !supported(e)
        || first.token_in != last.token_out
        || first.token_out != last.token_in
        || e.pool.reference != last.entry.pool.reference
        || legs[1..legs.len() - 1]
            .iter()
            .any(|l| l.entry.shares_liquidity(e))
    {
        return Err(bad());
    }
    let amount = num::to_units(input, first.dec_in).ok_or_else(bad)?;
    let next = if fixed_v4(e) {
        let PoolState::Cl {
            sqrt_price_x96,
            tick,
            liquidity,
            protocol_fee,
            lp_fee,
        } = &e.pool.state
        else {
            return Err(bad());
        };
        let zero = first.token_in == e.pool.token0;
        let state = ClState {
            sqrt_price: *sqrt_price_x96,
            tick: *tick,
            liquidity: *liquidity,
            fee: pool_quote::swap_fee(*protocol_fee, *lp_fee, zero),
            ticks: e.ticks.as_ref().ok_or_else(bad)?,
            version: Version::V4,
        };
        let (fill, next) = swap_cl::swap_final(&state, zero, true, amount).map_err(|_| bad())?;
        if !fill.complete {
            return Err(bad());
        }
        PoolState::Cl {
            sqrt_price_x96: next.sqrt_price,
            tick: next.tick,
            liquidity: next.liquidity,
            protocol_fee: *protocol_fee,
            lp_fee: *lp_fee,
        }
    } else {
        pool_after::apply(ctx, e, first.token_in, amount)?.1
    };
    let mut out = legs.to_vec();
    out.last_mut().ok_or_else(bad)?.entry.pool.state = next;
    Ok(out)
}

//! Сумма пробы одной неделимой акции и границы применения её результатов.
use super::side_fit::Trip;
use crate::{
    core::types::{PoolState, Side},
    venues::bsc_dex::pools::registry::Entry,
};
use alloy_primitives::U256;

pub(crate) fn one(e: &Entry, side: Side, cap: U256) -> Option<U256> {
    let PoolState::Brokerbot(s) = &e.pool.state else {
        return None;
    };
    let quote = match side {
        Side::Buy => s.swap(s.cash, false, U256::ONE)?.amount_in,
        Side::Sell => s.swap(s.share, true, U256::ONE)?.amount_out,
    };
    (!quote.is_zero() && quote <= cap).then_some(quote)
}
pub(crate) fn trip(e: &Entry, cap: U256) -> Option<Trip> {
    let PoolState::Brokerbot(s) = &e.pool.state else {
        return None;
    };
    let amount = one(e, Side::Buy, cap)?;
    let after = s.after(s.cash, amount)?;
    after.swap(s.share, true, U256::ONE)?;
    Some(Trip {
        amount,
        coins: U256::ONE,
    })
}

pub(super) fn check(
    ctx: &crate::venues::bsc_dex::ctx::Ctx,
    leg: &super::route_calc::Leg,
    swap: &crate::venues::bsc_dex::math::swap_cl::Swapped,
) -> crate::gateway::errors::GatewayResult<()> {
    if leg.usable.one_share_only
        && (if leg.token_in == leg.entry.coin {
            swap.amount_in
        } else {
            swap.amount_out
        }) != U256::ONE
    {
        return Err(ctx.err(
            crate::gateway::errors::ErrorCode::NoRoute,
            "Aktionariat: размер отличается от измеренной одной акции",
        ));
    }
    Ok(())
}

pub(super) fn split_unit(
    candidates: &[super::graph_split::Cand],
    side: Side,
    unit: rust_decimal::Decimal,
    qty: rust_decimal::Decimal,
) -> rust_decimal::Decimal {
    let whole = candidates.iter().any(|c| match side {
        Side::Buy => c.legs.last().is_some_and(|leg| {
            leg.entry.family.variant.issuer_shares()
                && leg.dec_out == 0
                && leg.token_out == leg.entry.coin
        }),
        Side::Sell => c.legs.first().is_some_and(|leg| {
            leg.entry.family.variant.issuer_shares()
                && leg.dec_in == 0
                && leg.token_in == leg.entry.coin
        }),
    });
    if whole { unit.ceil().min(qty) } else { unit }
}

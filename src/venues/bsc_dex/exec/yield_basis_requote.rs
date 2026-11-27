//! Rebuild a previously funded single-market order using its original raw input.
//! The working wallet measures the payment again at the current canonical parent.
use super::{executor, yield_basis_plan};
use crate::{
    core::types::{RouteHop, RouterFeeMode, Side},
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::{
        math::swap_cl::Swapped,
        num,
        pools::{registry::Entry, yield_basis_quotes},
        quote::{
            direct::{self, Candidate},
            path::Path,
            select,
            v2_wallet_sim::{self, Order, V5Step, Wire},
            via::Via,
            white_state,
        },
        wallet::signer,
    },
};
use alloy_primitives::{Address, Bytes, U256};
use rust_decimal::Decimal;

pub(in crate::venues::bsc_dex) struct OrderInput {
    target: Address,
    native: bool,
    amount: U256,
    via_v5: Option<Address>,
}
pub(in crate::venues::bsc_dex) fn selected(hops: &[RouteHop]) -> Option<OrderInput> {
    let [hop] = hops else {
        return None;
    };
    let Some(RouterFeeMode::YieldBasis {
        target,
        native,
        principal: Some(amount),
        v5,
        ..
    }) = hop.fee_mode
    else {
        return None;
    };
    Some(OrderInput {
        target,
        native,
        amount,
        via_v5: v5,
    })
}
pub(in crate::venues::bsc_dex) async fn path(
    v: &Via<'_>,
    e: &Entry,
    qty: Decimal,
    direct_mode: bool,
    OrderInput {
        target,
        native,
        amount,
        via_v5,
    }: OrderInput,
    block: Option<u64>,
) -> GatewayResult<Path> {
    let q = v.q;
    let ctx = &q.ctx;
    let bad = || {
        ctx.err(
            ErrorCode::QuoteMismatch,
            "YieldBasis: источник, выплата или получатель расходятся с исходным поручением",
        )
    };
    if direct_mode != via_v5.is_none() || amount.is_zero() {
        return Err(bad());
    }
    let t = yield_basis_plan::terms(ctx, e, target, native)?;
    let payer = signer::address(ctx)?;
    let tag = block
        .or_else(|| super::quoter_refetch::block(q))
        .map(|b| format!("{b:#x}"))
        .unwrap_or_else(|| "latest".to_owned());
    let h = white_state::head(ctx, &tag).await?;
    let at = h.selector();
    let (market, ()) = tokio::try_join!(
        yield_basis_quotes::bound(
            ctx,
            e.family,
            &e.pool.reference,
            (e.pool.token0, e.pool.token1),
            &at
        ),
        yield_basis_plan::check_module(ctx, e, t, &at)
    )?;
    let buy = v.side == Side::Buy;
    let (input, output) = if buy {
        (v.usdt, v.coin)
    } else {
        (v.coin, v.usdt)
    };
    let wanted = direct::ceil_units(qty, v.coin_dec).ok_or_else(bad)?;
    if wanted.is_zero()
        || (!buy && amount != wanted)
        || !market.coins().contains(&input)
        || !market.coins().contains(&output)
        || input == output
    {
        return Err(bad());
    }
    let (to, wire) = if let Some(exec) = via_v5 {
        if native || executor::address(ctx)? != Some(exec) {
            return Err(bad());
        }
        (
            exec,
            Wire::V5(V5Step {
                native: None,
                kind: 17,
                pool: target,
                data: Bytes::copy_from_slice(t.market.as_slice()),
            }),
        )
    } else {
        (target, Wire::YieldBasis(t))
    };
    let mut usable =
        select::usable(q, e, v.side, v.usdt_dec).map_err(|why| ctx.err(ErrorCode::NoRoute, why))?;
    let fill = v2_wallet_sim::measure(
        ctx,
        h,
        Order {
            payer,
            target: to,
            input,
            output,
            amount,
            minimum: if buy { wanted } else { U256::ONE },
            wire,
        },
        false,
    )
    .await?;
    if fill.charged != amount || (buy && fill.received < wanted) {
        return Err(bad());
    }
    let usdt = num::to_decimal(if buy { fill.charged } else { fill.received }, v.usdt_dec)
        .ok_or_else(bad)?;
    usable.gas = fill.swap_gas;
    Path::one(
        ctx,
        Candidate {
            entry: e.clone(),
            usable,
            swapped: Swapped {
                amount_in: fill.charged,
                amount_out: fill.received,
                complete: true,
            },
            coins: qty,
            usdt,
        },
        v.side,
        (v.coin_dec, v.usdt_dec),
    )
}

#[cfg(test)]
mod tests {
    use crate::core::types::RouterFeeMode;
    use alloy_primitives::{Address, U256};
    #[test]
    fn earlier_routes_and_exact_measured_sources_round_trip_independently() {
        let target = Address::with_last_byte(1);
        let old =
            serde_json::json!({"YieldBasis":{"target":target,"native":false,"native_cash":false}});
        let read = serde_json::from_value::<RouterFeeMode>(old.clone()).unwrap();
        assert!(matches!(
            read,
            RouterFeeMode::YieldBasis {
                principal: None,
                v5: None,
                ..
            }
        ));
        assert_eq!(serde_json::to_value(read).unwrap(), old);
        let amount = U256::from(10016450390026224122u64);
        for v5 in [None, Some(Address::with_last_byte(2))] {
            let chosen = RouterFeeMode::YieldBasis {
                target,
                native: false,
                native_cash: false,
                principal: Some(amount),
                v5,
            };
            assert_eq!(
                serde_json::from_slice::<RouterFeeMode>(&serde_json::to_vec(&chosen).unwrap())
                    .unwrap(),
                chosen
            );
        }
    }
}

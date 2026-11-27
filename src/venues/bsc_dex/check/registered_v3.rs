//! Проба официального V3 Router с фактическими средствами того же кошелька на закреплённой голове.
use super::super::{
    ctx::Ctx,
    exec::{
        plan::Order,
        registered_v3_wallet::{Market, RouterMarket},
    },
    node::{abi::IERC20, call, rpc},
    pools::{family_variant::Variant, registered_router, registry::Entry},
    quote::white_state::Head,
    wallet::approve::{self, Approval},
};
use crate::gateway::errors::{ErrorCode, GatewayResult};
use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256};
use serde_json::json;

pub(super) fn configured(ctx: &Ctx, e: &Entry) -> bool {
    e.family.variant.exact_router()
        && registered_router::eligible(ctx,e.family.id,e.coin,e.usdt)
        && registered_router::target(ctx,e).is_ok()
}
pub(super) async fn funded(
    ctx: &Ctx,
    e: &Entry,
    wallet: Address,
    amount: U256,
    head: Head,
) -> GatewayResult<()> {
    let at = head.selector();
    let data = Bytes::from(IERC20::balanceOfCall { owner: wallet }.abi_encode());
    let raw: Bytes = rpc::call(ctx, "eth_call", json!([{"to":e.usdt,"data":data},at])).await?;
    if call::decode::<IERC20::balanceOfCall>(&raw)? < amount {
        return Err(ctx.err(
            ErrorCode::InsufficientBalance,
            "V3 Router: на кошельке пробы недостаточно исходного ERC20",
        ));
    }
    Ok(())
}
pub(super) async fn router(ctx: &Ctx, e: &Entry, head: Head) -> GatewayResult<Address> {
    let at = head.selector();
    if matches!(e.family.variant,Variant::SolidlyProxy|Variant::SolidlyPairOverride) {
        Box::pin(super::super::pools::solidly_proxy::router(
            ctx, e, e.usdt, e.coin, &at,
        ))
        .await
    } else {
        registered_router::checked(ctx, e, e.usdt, e.coin, &at).await
    }
}
pub(super) fn leg(
    ctx: &Ctx,
    e: &Entry,
    router: Address,
    wallet: Address,
    order: Order,
    now: u64,
) -> GatewayResult<(Vec<Approval>, Address, Bytes)> {
    let fail = || {
        ctx.err(
            ErrorCode::NoRoute,
            "V3 Router: не подтверждены ставка или срок поручения",
        )
    };
    let end = U256::from(
        now.checked_add(ctx.int("dex.deadline_s")?)
            .ok_or_else(fail)?,
    );
    let pool = e.pool.reference.address();
    let d = if e.family.variant == Variant::V2ExactRouter {
        super::super::exec::solidly_router_wallet::Market{pool,stable:None,supporting_fee:false}
          .order(order.token_in,order.token_out,order.amount_in,order.min_out,wallet,end)
    } else if matches!(e.family.variant,Variant::SolidlyProxy|Variant::SolidlyPairOverride) {
        let crate::core::types::PoolState::Solidly { stable, .. } = &e.pool.state else {
            return Err(fail());
        };
        super::super::exec::solidly_router_wallet::Market {
            pool,
            stable: Some(*stable),
            supporting_fee:e.family.variant==Variant::SolidlyPairOverride,
        }
        .order(
            order.token_in,
            order.token_out,
            order.amount_in,
            order.min_out,
            wallet,
            end,
        )
    } else {
        Market::from_entry(e).ok_or_else(fail)?
        .order(
            order.token_in,
            order.token_out,
            order.amount_in,
            order.min_out,
            wallet,
            end,
        )
    };
    Ok((
        approve::fork_all(ctx, order.token_in, router, order.amount_in)?,
        router,
        d,
    ))
}

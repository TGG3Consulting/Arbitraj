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
    registered_router::wallet_module(ctx,e).is_some() || (e.family.variant.exact_router()
        && registered_router::eligible(ctx,e.family.id,e.coin,e.usdt)
        && registered_router::target(ctx,e).is_ok())
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
    if e.family.variant == Variant::MaverickTwoRouter
        && [e.coin, e.usdt].contains(&ctx.addr("dex.native_wrapped")?) {
        return Err(ctx.err(ErrorCode::NotApplicable, "Двусторонней пробе Maverick с оплатой WETH требуется V5"));
    }
    if registered_router::wallet_module(ctx,e).is_some() && let Ok(m)=registered_router::checked_module(ctx,e,&at).await{return Ok(m)}
    if e.family.variant.classic_wallet() {
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
    let d=if e.family.variant.classic_wallet() || matches!(e.family.variant,Variant::V2ExactRouter|Variant::EmpireNominal) {
        super::super::exec::solidly_router_wallet::Market::from_entry(ctx,e)?.order(
            order.token_in,order.token_out,order.amount_in,order.min_out,wallet,end)
    } else {
        let mut m=Market::from_entry(ctx,e).ok_or_else(fail)?;
        m.wallet_module=registered_router::wallet_module(ctx,e)==Some(router);
        m.order(
            order.token_in,
            order.token_out,
            order.amount_in,
            order.min_out,
            wallet,
            end,
        )
    };
    Ok((
        if e.family.variant==Variant::SlipstreamUniversal {
            approve::all(ctx,order.token_in,ctx.dex_addr("dex.permit2",e.family.id)?,router,order.amount_in,now)?
        }else{approve::fork_all(ctx, order.token_in, router, order.amount_in)?},
        router,
        d,
    ))
}

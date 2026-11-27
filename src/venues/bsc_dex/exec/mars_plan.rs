//! Публикуемая плата MarSwap за одну пару. Каждая форма проверяется по той же фабрике и коду Router02.
use super::{
    chain::Step,
    mars_wallet::Terms,
    plan::{DirectCall, Plan},
};
use crate::{
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::{
        ctx::Ctx,
        node::rpc,
        pools::{dagrow, registry::Entry, v2_live_fee},
        quote::{
            router_pricing, v2_wallet_sim as sim,
            white_state::{self, Head},
        },
        wallet::approve,
    },
};
use alloy_primitives::{Address, Bytes, U256};
use serde_json::json;
pub(crate) fn applicable(ctx: &Ctx, e: &Entry) -> bool {
    e.family.variant == crate::venues::bsc_dex::pools::family_variant::Variant::V2DualCreate2LiveFee
        && ctx
            .dex_text("dex.v2_fee_method", e.family.id)
            .is_ok_and(|s| s == "lpInfo()")
}
fn bind(ctx: &Ctx, e: &Entry, wei: U256) -> GatewayResult<Terms> {
    let (router, router_hash) = router_pricing::profile(ctx, e.family.id)?;
    let pair_hash = crate::venues::bsc_dex::pools::v2_pair_binding::expected(
        ctx,
        e.family.id,
        "dex.pair_runtime_hash",
    )?;
    Ok(Terms {
        pair: e.pool.reference.address(),
        router,
        wei,
        router_hash,
        pair_hash,
    })
}
pub(crate) async fn terms(ctx: &Ctx, e: &Entry, h: Head) -> GatewayResult<Terms> {
    let bad = || {
        ctx.err(
            ErrorCode::NoRoute,
            "MarSwap: не подтверждены одношаговая плата фабрики и роутер",
        )
    };
    if !applicable(ctx, e) {
        return Err(bad());
    }
    let market = v2_live_fee::Market {
        pool: e.pool.reference.address(),
        tokens: (e.pool.token0, e.pool.token1),
    };
    let rate = v2_live_fee::Rate::configured(ctx, e.family, market)?;
    let (factory, data) = rate.call();
    let at = h.selector();
    let (bytes, ()): (Bytes, ()) = tokio::try_join!(
        rpc::call(ctx, "eth_call", json!([{"to":factory,"data":data},at])),
        router_pricing::registered(ctx, e, h)
    )?;
    let wei = rate
        .flat(&bytes)
        .filter(|fee| !fee.is_zero())
        .ok_or_else(bad)?;
    bind(ctx, e, wei)
}
pub(crate) fn observed(ctx: &Ctx, e: &Entry) -> Option<U256> {
    let factory = ctx.dex_addr("dex.factory", e.family.id).ok()?;
    crate::venues::bsc_dex::pools::mars_fee::indicative(ctx.chain_id()?, factory, e.pool.block)
}
pub(crate) async fn candidate(ctx: &Ctx, e: &Entry, head: Option<Head>) -> Option<Terms> {
    if let Some(h) = head {
        return terms(ctx, e, h).await.ok();
    }
    let wei = observed(ctx, e).filter(|fee| !fee.is_zero())?;
    bind(ctx, e, wei).ok()
}
pub(crate) async fn plan(
    ctx: &Ctx,
    s: &Step,
    payer: Address,
    height: u64,
    expected: (Address, U256),
) -> GatewayResult<Plan> {
    let bad = || {
        ctx.err(
            ErrorCode::NoRoute,
            "MarSwap: изменилась фиксированная плата или параметры кошелька",
        )
    };
    if !s.from_wallet || !s.to_wallet {
        return Err(bad());
    }
    let g = ctx.settings.generation();
    let h = white_state::head(ctx, &format!("{height:#x}")).await?;
    let t = terms(ctx, &s.entry, h).await?;
    if (t.router, t.wei) != expected {
        return Err(bad());
    }
    let o = &s.order;
    let order = sim::Order {
        payer,
        target: t.router,
        input: o.token_in,
        output: o.token_out,
        amount: o.amount_in,
        minimum: o.min_out,
        wire: sim::Wire::NativeFlat(t),
    };
    let deadline = U256::from(
        h.timestamp
            .to::<u64>()
            .checked_add(ctx.int("dex.deadline_s")?)
            .ok_or_else(bad)?,
    );
    let proof = t.proof(&order, o.min_out, deadline);
    sim::measure(ctx, h, order, true).await?;
    let current = dagrow::allowance(ctx, o.token_in, payer, t.router, &h.selector()).await?;
    let approvals = approve::fork_exact(ctx, o.token_in, t.router, Some(current), o.amount_in)?;
    if ctx.settings.generation() != g {
        return Err(bad());
    }
    let mut plan = Plan::new(
        ctx,
        (approvals, t.router, payer),
        o.token_out,
        vec![],
        o.min_out,
        deadline,
    )?;
    plan.set_direct_market(DirectCall::NativeFlat(std::sync::Arc::new(proof)));
    Ok(plan)
}

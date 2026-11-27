//! Построение прямой транзакции и шага V5 для конкретного пула и его исходной валюты.
use super::{
    chain::Step,
    mooniswap_wallet::{Proof, Terms, abi::IModule},
    plan::{DirectCall, Plan},
};
use crate::{
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::{
        ctx::Ctx,
        node::call::canonical,
        pools::{dagrow, direct_module, mooniswap, registry::Entry},
        quote::{v2_wallet_sim as sim, white_state},
        wallet::approve,
    },
};
use alloy_primitives::{Address, Bytes, U256};
use serde_json::Value;

fn bad(ctx: &Ctx) -> crate::gateway::errors::GatewayError {
    ctx.err(
        ErrorCode::NoRoute,
        "Mooniswap: рынок, валюты или модуль кошелька не совпали",
    )
}
pub(crate) fn key(ctx: &Ctx, e: &Entry, input: Address) -> Option<alloy_primitives::B256> {
    let wrapped = ctx.addr("dex.native_wrapped").ok()?;
    let raw = mooniswap::shape(e, wrapped)?;
    let norm = |a: Address| if a.is_zero() { wrapped } else { a };
    let idx = raw.iter().position(|a| norm(*a) == input)?;
    let crate::core::types::PoolRef::Contract(pool) = e.pool.reference else {
        return None;
    };
    let mut key = pool.into_word();
    key[11] = if raw[idx].is_zero() {
        1
    } else if raw[1 - idx].is_zero() {
        2
    } else {
        0
    };
    Some(key)
}
pub(in crate::venues::bsc_dex) fn graph_target(
    ctx: &Ctx,
    e: &Entry,
    input: Address,
) -> Option<(alloy_primitives::B256, Address)> {
    let t = terms(
        ctx,
        e,
        input,
        ctx.dex_addr("dex.router", e.family.id).ok()?,
        false,
    )
    .ok()?;
    Some((t.market, t.target))
}
pub(crate) fn terms(
    ctx: &Ctx,
    e: &Entry,
    input: Address,
    target: Address,
    native: bool,
) -> GatewayResult<Terms> {
    let market = key(ctx, e, input).ok_or_else(|| bad(ctx))?;
    let pins = direct_module::configured(ctx, e.family.id)?;
    let [module_hash] = pins.as_slice() else {
        return Err(bad(ctx));
    };
    let wrapped = ctx.addr("dex.native_wrapped")?;
    let crate::core::types::PoolState::Mooniswap(state) = &e.pool.state else {
        return Err(bad(ctx));
    };
    if !e.family.variant.mooniswap()
        || target != ctx.dex_addr("dex.router", e.family.id)?
        || target.is_zero()
        || native && ![e.pool.token0, e.pool.token1].contains(&wrapped)
    {
        return Err(bad(ctx));
    }
    Ok(Terms {
        market,
        target,
        wrapped,
        native,
        successor: state.successor,
        module_hash: *module_hash,
    })
}
pub(crate) async fn check_module(ctx: &Ctx, e: &Entry, t: Terms, at: &Value) -> GatewayResult<()> {
    let ((), factory, wrapped, successor, raw) = tokio::try_join!(
        direct_module::verify_code(ctx, e.family.id, t.target, at),
        canonical(ctx, at, t.target, IModule::factoryCall {}),
        canonical(ctx, at, t.target, IModule::wrappedCall {}),
        canonical(ctx, at, t.target, IModule::hasSwapForCall {}),
        mooniswap::market(ctx, e.family, e.pool.reference.address(), at)
    )?;
    if factory != ctx.dex_addr("dex.factory", e.family.id)?
        || wrapped != t.wrapped
        || successor != t.successor
        || Some(raw) != mooniswap::shape(e, t.wrapped)
    {
        return Err(bad(ctx));
    }
    Ok(())
}
pub(in crate::venues::bsc_dex) async fn check_step(
    ctx: &Ctx,
    e: &Entry,
    input: Address,
    at: &Value,
) -> GatewayResult<()> {
    let t = terms(
        ctx,
        e,
        input,
        ctx.dex_addr("dex.router", e.family.id)?,
        false,
    )?;
    check_module(ctx, e, t, at).await
}
pub(super) async fn plan(
    ctx: &Ctx,
    s: &Step,
    payer: Address,
    height: u64,
    pick: Option<(Address, bool)>,
) -> GatewayResult<Plan> {
    let generation = ctx.settings.generation();
    if !s.from_wallet || !s.to_wallet {
        return Err(bad(ctx));
    }
    let (target, native) = pick.ok_or_else(|| bad(ctx))?;
    let e = &s.entry;
    let t = terms(ctx, e, s.order.token_in, target, native)?;
    let o = &s.order;
    let h = white_state::head(ctx, &format!("{height:#x}")).await?;
    let at = h.selector();
    check_module(ctx, e, t, &at).await?;
    let order = sim::Order {
        payer,
        target,
        input: o.token_in,
        output: o.token_out,
        amount: o.amount_in,
        minimum: o.min_out,
        wire: sim::Wire::Mooniswap(t),
    };
    let deadline = U256::from(
        h.timestamp
            .to::<u64>()
            .checked_add(ctx.int("dex.deadline_s")?)
            .ok_or_else(|| bad(ctx))?,
    );
    let proof = t.proof(&order, o.min_out, deadline);
    sim::measure(ctx, h, order, true).await?;
    let approvals = if t.value(o.token_in, o.amount_in).is_zero() {
        let current = dagrow::allowance(ctx, o.token_in, payer, target, &at).await?;
        approve::fork_exact(ctx, o.token_in, target, Some(current), o.amount_in)?
    } else {
        vec![]
    };
    if ctx.settings.generation() != generation {
        return Err(bad(ctx));
    }
    let mut plan = Plan::new(
        ctx,
        (approvals, target, payer),
        o.token_out,
        vec![],
        o.min_out,
        deadline,
    )?;
    plan.set_direct_market(DirectCall::Mooniswap(std::sync::Arc::new(proof)));
    Ok(plan)
}
pub(crate) fn probe(
    ctx: &Ctx,
    e: &Entry,
    o: &super::plan::Order,
    payer: Address,
    now: u64,
) -> GatewayResult<(Vec<approve::Approval>, Address, Bytes)> {
    let target = ctx.dex_addr("dex.router", e.family.id)?;
    let t = terms(ctx, e, o.token_in, target, false)?;
    let proof = Proof {
        terms: t,
        payer,
        input: o.token_in,
        output: o.token_out,
        maximum: o.amount_in,
        minimum: o.min_out,
        deadline: U256::from(
            now.checked_add(ctx.int("dex.deadline_s")?)
                .ok_or_else(|| bad(ctx))?,
        ),
    };
    Ok((
        approve::fork_exact(ctx, o.token_in, target, None, o.amount_in)?,
        target,
        proof.data(),
    ))
}

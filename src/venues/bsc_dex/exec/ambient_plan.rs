use super::{
    ambient_wallet::{Terms, abi::IWallet},
    chain::Step,
    plan::{DirectCall, Plan},
};
use crate::{
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::{
        ctx::Ctx,
        node::call::canonical,
        pools::{ambient, dagrow, direct_module, registry::Entry},
        quote::{v2_wallet_sim as sim, white_state},
        wallet::approve,
    },
};
use alloy_primitives::{Address, U256};
use serde_json::Value;
fn bad(ctx: &Ctx) -> crate::gateway::errors::GatewayError {
    ctx.err(
        ErrorCode::NoRoute,
        "Ambient: не подтверждены модуль, предел суммы или регистрация рынка",
    )
}
pub(crate) fn terms(
    ctx: &Ctx,
    e: &Entry,
    target: Address,
    desired: Option<U256>,
) -> GatewayResult<Terms> {
    let s = ambient::shape(e).ok_or_else(|| bad(ctx))?;
    let pins = direct_module::configured(ctx, e.family.id)?;
    let [module_hash] = pins.as_slice() else {
        return Err(bad(ctx));
    };
    if target.is_zero()
        || target != ctx.dex_addr("dex.router", e.family.id)?
        || desired == Some(U256::ZERO)
    {
        return Err(bad(ctx));
    }
    Ok(Terms {
        market: s.market,
        target,
        module_hash: *module_hash,
        desired,
    })
}
pub(crate) async fn check_module(ctx: &Ctx, e: &Entry, t: Terms, at: &Value) -> GatewayResult<()> {
    let r = ambient::check(ctx, e, at).await?;
    let target = t.target;
    let ((), dex, query, wrapped, hot, ko, dh, hh, kh) = tokio::try_join!(
        direct_module::verify_code(ctx, e.family.id, target, at),
        canonical(ctx, at, target, IWallet::dexCall {}),
        canonical(ctx, at, target, IWallet::queryCall {}),
        canonical(ctx, at, target, IWallet::wrappedCall {}),
        canonical(ctx, at, target, IWallet::hotCall {}),
        canonical(ctx, at, target, IWallet::knockoutCall {}),
        canonical(ctx, at, target, IWallet::venueHashCall {}),
        canonical(ctx, at, target, IWallet::hotHashCall {}),
        canonical(ctx, at, target, IWallet::knockoutHashCall {})
    )?;
    if (dex, query, wrapped, hot, ko, dh, hh, kh)
        != (
            r.dex,
            r.query,
            ctx.addr("dex.native_wrapped")?,
            r.hot,
            r.knockout,
            r.hashes[0],
            r.hashes[3],
            r.hashes[4],
        )
    {
        return Err(bad(ctx));
    }
    Ok(())
}
pub(crate) async fn graph(ctx: &Ctx, e: &Entry, input: Address, at: &Value) -> GatewayResult<()> {
    let target = ctx.dex_addr("dex.router", e.family.id)?;
    let t = terms(ctx, e, target, None)?;
    let output = if e.pool.token0 == input {
        e.pool.token1
    } else if e.pool.token1 == input {
        e.pool.token0
    } else {
        return Err(bad(ctx));
    };
    check_module(ctx, e, t, at).await?;
    let m = canonical(
        ctx,
        at,
        target,
        IWallet::resolveCall {
            id: t.market.id(),
            src: input,
            dst: output,
        },
    )
    .await?;
    if (m.base, m.quote, m.index) != (t.market.base, t.market.quote, t.market.index) {
        return Err(bad(ctx));
    }
    Ok(())
}
pub(super) async fn plan(
    ctx: &Ctx,
    s: &Step,
    payer: Address,
    height: u64,
    selection: Option<(Address, Option<U256>)>,
) -> GatewayResult<Plan> {
    let before = ctx.settings.generation();
    if !s.from_wallet || !s.to_wallet {
        return Err(bad(ctx));
    }
    let (target, desired) = selection.ok_or_else(|| bad(ctx))?;
    let e = &s.entry;
    let o = &s.order;
    if desired.is_some_and(|w| w < o.min_out) {
        return Err(bad(ctx));
    }
    let terms = terms(ctx, e, target, desired)?;
    let h = white_state::head(ctx, &format!("{height:#x}")).await?;
    let at = h.selector();
    check_module(ctx, e, terms, &at).await?;
    let order = sim::Order {
        payer,
        target,
        input: o.token_in,
        output: o.token_out,
        amount: o.amount_in,
        minimum: o.min_out,
        wire: sim::Wire::Ambient(terms),
    };
    let end = U256::from(
        h.timestamp
            .to::<u64>()
            .checked_add(ctx.int("dex.deadline_s")?)
            .ok_or_else(|| bad(ctx))?,
    );
    let proof = terms.proof(&order, o.min_out, end);
    sim::measure(ctx, h, order, true).await?;
    let grant = dagrow::allowance(ctx, o.token_in, payer, target, &at).await?;
    let approvals = approve::fork_exact(ctx, o.token_in, target, Some(grant), o.amount_in)?;
    if ctx.settings.generation() != before {
        return Err(bad(ctx));
    }
    let mut plan = Plan::new(
        ctx,
        (approvals, target, payer),
        o.token_out,
        vec![],
        o.min_out,
        end,
    )?;
    plan.set_direct_market(DirectCall::Ambient(std::sync::Arc::new(proof)));
    Ok(plan)
}

pub(in crate::venues::bsc_dex) fn probe(
    ctx: &Ctx,
    e: &Entry,
    order: &super::plan::Order,
    payer: Address,
    now: u64,
) -> GatewayResult<(Vec<approve::Approval>, Address, alloy_primitives::Bytes)> {
    let target = ctx.dex_addr("dex.router", e.family.id)?;
    let terms = terms(ctx, e, target, None)?;
    let end = U256::from(
        now.checked_add(ctx.int("dex.deadline_s")?)
            .ok_or_else(|| bad(ctx))?,
    );
    let wire = sim::Order {
        payer,
        target,
        input: order.token_in,
        output: order.token_out,
        amount: order.amount_in,
        minimum: order.min_out,
        wire: sim::Wire::Ambient(terms),
    };
    Ok((
        approve::fork_exact(ctx, order.token_in, target, None, order.amount_in)?,
        target,
        terms.proof(&wire, order.min_out, end).data(),
    ))
}

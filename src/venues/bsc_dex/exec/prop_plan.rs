//! Подписываемая одиночная заявка LambdaClass для реальной пары и текущего кошелька.
use super::{
    chain::Step,
    plan::{DirectCall, Plan},
    prop_wallet::{Proof, Terms},
};
use crate::{
    gateway::errors::GatewayResult,
    venues::bsc_dex::{
        ctx::Ctx,
        pools::{dagrow, exact_wallet_tokens, prop_venue as prop, registry::Entry},
        quote::{v2_wallet_sim as sim, white_state},
        wallet::approve,
    },
};
use alloy_primitives::{Address, Bytes, U256};
use serde_json::Value;
pub(crate) async fn checked(
    ctx: &Ctx,
    e: &Entry,
    i: Address,
    o: Address,
    target: Address,
    fallback_allowed: bool,
    at: &Value,
) -> GatewayResult<Terms> {
    let (r, on) = prop::state(ctx, e.family, e.pool.reference.address(), (i, o), at).await?;
    if !on {
        return Err(prop::bad(ctx));
    }
    if target == r.router() {
        if !fallback_allowed {
            return Err(prop::bad(ctx));
        }
        exact_wallet_tokens::check_input(ctx, e.family.id, i, at).await?
    } else {
        prop::module(ctx, e.family, &r, target, at).await?
    }
    r.terms(ctx, target, fallback_allowed)
}
pub(super) async fn plan(
    ctx: &Ctx,
    s: &Step,
    payer: Address,
    height: u64,
    selection: Option<(Address, bool)>,
) -> GatewayResult<Plan> {
    let g = ctx.settings.generation();
    let (target, fallback_allowed) = selection.ok_or_else(|| prop::bad(ctx))?;
    let o = &s.order;
    let e = &s.entry;
    if !s.from_wallet || !s.to_wallet {
        return Err(prop::bad(ctx));
    }
    let head = white_state::head(ctx, &format!("{height:#x}")).await?;
    let at = head.selector();
    let t = checked(
        ctx,
        e,
        o.token_in,
        o.token_out,
        target,
        fallback_allowed,
        &at,
    )
    .await?;
    let order = sim::Order {
        payer,
        target,
        input: o.token_in,
        output: o.token_out,
        amount: o.amount_in,
        minimum: o.min_out,
        wire: sim::Wire::Prop(Box::new(t)),
    };
    let deadline = U256::from(
        head.timestamp
            .to::<u64>()
            .checked_add(ctx.int("dex.deadline_s")?)
            .ok_or_else(|| prop::bad(ctx))?,
    );
    let p = t.proof(&order, o.min_out, deadline);
    sim::measure(ctx, head, order, true).await?;
    let approvals = if target == t.router {
        let a = dagrow::allowance(ctx, o.token_in, payer, target, &at).await?;
        approve::fork_exact(ctx, o.token_in, target, Some(a), o.amount_in)?
    } else {
        approve::fork_missing(ctx, o.token_in, payer, target, o.amount_in).await?
    };
    if g != ctx.settings.generation() {
        return Err(prop::bad(ctx));
    }
    let mut plan = Plan::new(
        ctx,
        (approvals, target, payer),
        o.token_out,
        vec![],
        o.min_out,
        deadline,
    )?;
    plan.set_direct_market(DirectCall::Prop(std::sync::Arc::new(p)));
    Ok(plan)
}
pub(crate) fn probe_target(ctx: &Ctx, e: &Entry, input: Address) -> GatewayResult<Address> {
    if exact_wallet_tokens::eligible_input(ctx, e.family.id, input) {
        return ctx.dex_addr("dex.quoter", e.family.id);
    }
    ctx.dex_addr("dex.router", e.family.id)
}
pub(crate) fn probe(
    ctx: &Ctx,
    e: &Entry,
    o: &super::plan::Order,
    payer: Address,
    now: u64,
) -> GatewayResult<(Vec<approve::Approval>, Address, Bytes)> {
    let target = probe_target(ctx, e, o.token_in)?;
    let r = prop::Roles::read(ctx, e.family.id)?;
    if target == r.router() && !exact_wallet_tokens::eligible_input(ctx, e.family.id, o.token_in) {
        return Err(prop::bad(ctx));
    }
    let p = Proof {
        terms: r.terms(ctx, target, true)?,
        payer,
        input: o.token_in,
        output: o.token_out,
        amount: o.amount_in,
        minimum: o.min_out,
        deadline: U256::from(
            now.checked_add(ctx.int("dex.deadline_s")?)
                .ok_or_else(|| prop::bad(ctx))?,
        ),
    };
    Ok((
        approve::fork_exact(ctx, o.token_in, target, None, o.amount_in)?,
        target,
        p.data(),
    ))
}

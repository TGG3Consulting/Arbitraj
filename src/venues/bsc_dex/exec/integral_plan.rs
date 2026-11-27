//! Подготовка одной мгновенной заявки на остатках кошелька и живой реализации Integral SIZE.
use super::{
    chain::Step,
    integral_wallet::{Proof, Terms},
    plan::{DirectCall, Plan},
};
use crate::{
    gateway::errors::GatewayResult,
    venues::bsc_dex::{
        ctx::Ctx,
        pools::{
            dagrow, exact_wallet_tokens,
            integral_size::{self, Roles},
            registry::Entry,
        },
        quote::{v2_wallet_sim as sim, white_state},
        wallet::approve,
    },
};
use alloy_primitives::{Address, Bytes, U256};
use serde_json::Value;
pub(crate) fn terms(e: &Entry, r: Roles, target: Address, desired: Option<U256>) -> Terms {
    Terms {
        pair: e.pool.reference.address(),
        target,
        relay: r.relay,
        delay: r.delay,
        code: r.code_identity(),
        desired,
    }
}
pub(crate) async fn checked(
    ctx: &Ctx,
    e: &Entry,
    input: Address,
    output: Address,
    target: Address,
    at: &Value,
) -> GatewayResult<Roles> {
    let (r, on) = integral_size::state(
        ctx,
        e.family,
        e.pool.reference.address(),
        (input.min(output), output.max(input)),
        at,
    )
    .await?;
    if !on || !integral_size::funded(ctx, r, at).await? {
        return Err(integral_size::bad(ctx));
    }
    if target == r.relay {
        exact_wallet_tokens::check(ctx, e.family.id, input, output, at).await?
    } else if e.family.executor_kind.is_none()
        || integral_size::module(ctx, e.family, r, at).await? != target
    {
        return Err(integral_size::bad(ctx));
    }
    Ok(r)
}
pub(super) async fn plan(
    ctx: &Ctx,
    s: &Step,
    payer: Address,
    height: u64,
    selection: Option<(Address, Option<U256>)>,
) -> GatewayResult<Plan> {
    let g = ctx.settings.generation();
    let (target, desired) = selection.ok_or_else(|| integral_size::bad(ctx))?;
    let (e, o) = (&s.entry, &s.order);
    if !s.from_wallet || !s.to_wallet {
        return Err(integral_size::bad(ctx));
    }
    let head = white_state::head(ctx, &format!("{height:#x}")).await?;
    let at = head.selector();
    let r = checked(ctx, e, o.token_in, o.token_out, target, &at).await?;
    let t = terms(e, r, target, desired);
    let order = sim::Order {
        payer,
        target,
        input: o.token_in,
        output: o.token_out,
        amount: o.amount_in,
        minimum: o.min_out,
        wire: sim::Wire::Integral(t),
    };
    let deadline = U256::from(
        head.timestamp
            .to::<u64>()
            .checked_add(ctx.int("dex.deadline_s")?)
            .filter(|v| u32::try_from(*v).is_ok())
            .ok_or_else(|| integral_size::bad(ctx))?,
    );
    let p = t.proof(&order, o.min_out, deadline);
    sim::measure(ctx, head, order, true).await?;
    let approvals = if target == r.relay {
        let current = dagrow::allowance(ctx, o.token_in, payer, target, &at).await?;
        approve::fork_exact(ctx, o.token_in, target, Some(current), o.amount_in)?
    } else {
        approve::fork_missing(ctx, o.token_in, payer, target, o.amount_in).await?
    };
    if g != ctx.settings.generation() {
        return Err(integral_size::bad(ctx));
    }
    let mut plan = Plan::new(
        ctx,
        (approvals, target, payer),
        o.token_out,
        vec![],
        o.min_out,
        deadline,
    )?;
    plan.set_direct_market(DirectCall::Integral(std::sync::Arc::new(p)));
    Ok(plan)
}
pub(crate) fn probe_target(
    ctx: &Ctx,
    e: &Entry,
    input: Address,
    output: Address,
) -> GatewayResult<Address> {
    if e.family.executor_kind.is_some()
        && let Ok(addr) = ctx.dex_addr("dex.router", e.family.id)
    {
        return Ok(addr);
    }
    if !exact_wallet_tokens::eligible(ctx, e.family.id, input, output) {
        return Err(integral_size::bad(ctx));
    }
    Ok(Roles::read(ctx, e.family.id)?.relay)
}
pub(crate) fn probe(
    ctx: &Ctx,
    e: &Entry,
    o: &super::plan::Order,
    payer: Address,
    now: u64,
) -> GatewayResult<(Vec<approve::Approval>, Address, Bytes)> {
    let r = Roles::read(ctx, e.family.id)?;
    let target = probe_target(ctx, e, o.token_in, o.token_out)?;
    let t = terms(e, r, target, None);
    let deadline = U256::from(
        now.checked_add(ctx.int("dex.deadline_s")?)
            .filter(|v| u32::try_from(*v).is_ok())
            .ok_or_else(|| integral_size::bad(ctx))?,
    );
    let p = Proof {
        terms: t,
        payer,
        input: o.token_in,
        output: o.token_out,
        amount: o.amount_in,
        minimum: o.min_out,
        deadline,
    };
    Ok((
        approve::fork_exact(ctx, o.token_in, target, None, o.amount_in)?,
        target,
        p.data(),
    ))
}

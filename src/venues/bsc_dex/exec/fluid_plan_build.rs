//! Собирает одиночный или составной план и привязывает подтверждения, разрешения и минимум кошелька.
use super::{
    chain::Step,
    fluid_lite_plan::{bad, check_module, hop_terms, terms},
    plan::{DirectCall, Plan},
};
use crate::{
    gateway::errors::GatewayResult,
    venues::bsc_dex::{
        ctx::Ctx,
        pools::dagrow,
        quote::{v2_wallet_sim as sim, white_state},
        wallet::approve,
    },
};
use alloy_primitives::{Address, U256};

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
    let terms = check_module(ctx, e, terms, &at).await?;
    let order = sim::Order {
        payer,
        target,
        input: o.token_in,
        output: o.token_out,
        amount: o.amount_in,
        minimum: o.min_out,
        wire: sim::Wire::FluidDexLite(terms),
    };
    finish(ctx, before, h, order).await
}
pub(super) async fn finish(
    ctx: &Ctx,
    before: u64,
    h: white_state::Head,
    order: sim::Order,
) -> GatewayResult<Plan> {
    let at = h.selector();
    let end = U256::from(
        h.timestamp
            .to::<u64>()
            .checked_add(ctx.int("dex.deadline_s")?)
            .ok_or_else(|| bad(ctx))?,
    );
    let proof = match &order.wire {
        sim::Wire::FluidDexLite(t) => {
            DirectCall::FluidDexLite(std::sync::Arc::new(t.proof(&order, order.minimum, end)))
        }
        sim::Wire::FluidHop(t, r) => {
            DirectCall::FluidDexLite(std::sync::Arc::new(r.proof(*t, &order, order.minimum, end)))
        }
        sim::Wire::Motoswap(t) => {
            DirectCall::Motoswap(std::sync::Arc::new(t.proof(&order, order.minimum, end)))
        }
        sim::Wire::MotoswapPath(t, r) => {
            DirectCall::Motoswap(std::sync::Arc::new(r.proof(*t, &order, order.minimum, end)))
        }
        _ => return Err(bad(ctx)),
    };
    let (input, output, amount, minimum, payer, target) = (
        order.input,
        order.output,
        order.amount,
        order.minimum,
        order.payer,
        order.target,
    );
    sim::measure(ctx, h, order, true).await?;
    let grant = dagrow::allowance(ctx, input, payer, target, &at).await?;
    let approvals = approve::fork_exact(ctx, input, target, Some(grant), amount)?;
    if ctx.settings.generation() != before {
        return Err(bad(ctx));
    }
    let mut plan = Plan::new(
        ctx,
        (approvals, target, payer),
        output,
        vec![],
        minimum,
        end,
    )?;
    plan.set_direct_market(proof);
    Ok(plan)
}

pub(super) async fn hop_plan(
    ctx: &Ctx,
    steps: &[Step],
    payer: Address,
    height: u64,
    (target, principal): (Address, U256),
) -> GatewayResult<Plan> {
    let before = ctx.settings.generation();
    let first = steps.first().ok_or_else(|| bad(ctx))?;
    let last = steps.last().ok_or_else(|| bad(ctx))?;
    if !first.from_wallet
        || !last.to_wallet
        || principal.is_zero()
        || principal > first.order.amount_in
        || steps
            .windows(2)
            .any(|s| s[0].order.token_out != s[1].order.token_in)
    {
        return Err(bad(ctx));
    }
    let head = white_state::head(ctx, &format!("{height:#x}")).await?;
    let entries = steps.iter().map(|s| &s.entry).collect::<Vec<_>>();
    let path = std::iter::once(first.order.token_in)
        .chain(steps.iter().map(|s| s.order.token_out))
        .collect();
    let at = head.selector();
    let wire = if super::motoswap_plan::shape(&first.entry).is_some() {
        super::motoswap_plan::path_wire(ctx, &entries, target, path, &at)
            .await?
            .0
    } else {
        let (t, r) = hop_terms(ctx, &entries, target, path, &at).await?;
        sim::Wire::FluidHop(t, std::sync::Arc::new(r))
    };
    let order = sim::Order {
        payer,
        target,
        input: first.order.token_in,
        output: last.order.token_out,
        amount: principal,
        minimum: last.order.min_out,
        wire,
    };
    finish(ctx, before, head, order).await
}

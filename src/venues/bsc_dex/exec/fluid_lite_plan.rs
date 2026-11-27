use super::{
    chain::Step,
    fluid_lite_wallet::{Terms, abi::IWallet},
    plan::{DirectCall, Plan},
};
use crate::{
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::{
        ctx::Ctx,
        node::call::canonical,
        pools::{dagrow, direct_module, fluid_dex_lite as lite, registry::Entry},
        quote::{v2_wallet_sim as sim, white_state},
        wallet::approve,
    },
};
use alloy_primitives::{Address, U256};
use serde_json::Value;
fn bad(ctx: &Ctx) -> crate::gateway::errors::GatewayError {
    ctx.err(
        ErrorCode::NoRoute,
        "Fluid DEX Lite: не подтверждены модуль, предел суммы или регистрация рынка",
    )
}
pub(crate) fn terms(
    ctx: &Ctx,
    e: &Entry,
    target: Address,
    desired: Option<U256>,
) -> GatewayResult<Terms> {
    let s = lite::shape(e).ok_or_else(|| bad(ctx))?;
    let pins = direct_module::configured(ctx, e.family.id)?;
    let Some(module_hash) = pins.first() else {
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
        dex: s.dex,
        wrapped: s.wrapped,
        target,
        module_hash: *module_hash,
        desired,
    })
}
pub(crate) async fn check_module(
    ctx: &Ctx,
    e: &Entry,
    t: Terms,
    at: &Value,
) -> GatewayResult<Terms> {
    let r = lite::check(ctx, e, at).await?;
    let target = t.target;
    let (module_hash, dex, wrapped, hash, market) = tokio::try_join!(
        direct_module::verified_hash(ctx, e.family.id, target, at),
        canonical(ctx, at, target, IWallet::dexCall {}),
        canonical(ctx, at, target, IWallet::wrappedCall {}),
        canonical(ctx, at, target, IWallet::venueHashCall {}),
        canonical(
            ctx,
            at,
            target,
            IWallet::resolveCall {
                id: t.market.id().ok_or_else(|| bad(ctx))?
            }
        )
    )?;
    if (dex, wrapped, hash) != (r.core, t.wrapped, r.hash)
        || (market.token0, market.token1, market.salt)
            != (t.market.token0, t.market.token1, t.market.salt)
    {
        return Err(bad(ctx));
    }
    Ok(Terms { module_hash, ..t })
}
pub(crate) async fn graph(ctx: &Ctx, e: &Entry, input: Address, at: &Value) -> GatewayResult<()> {
    let target = ctx.dex_addr("dex.router", e.family.id)?;
    let t = terms(ctx, e, target, None)?;
    if !t
        .market
        .tokens(t.wrapped)
        .is_some_and(|v| v.contains(&input))
    {
        return Err(bad(ctx));
    }
    check_module(ctx, e, t, at).await?;
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
        wire: sim::Wire::FluidDexLite(terms),
    };
    Ok((
        approve::fork_exact(ctx, order.token_in, target, None, order.amount_in)?,
        target,
        terms.proof(&wire, order.min_out, end).data(),
    ))
}

pub(crate) async fn hop_terms(
    ctx: &Ctx,
    entries: &[&Entry],
    target: Address,
    path: Vec<Address>,
    at: &Value,
) -> GatewayResult<(Terms, super::fluid_lite_hop::Route)> {
    let first = *entries.first().ok_or_else(|| bad(ctx))?;
    let seed = terms(ctx, first, target, None)?;
    let (base, version) = tokio::try_join!(
        check_module(ctx, first, seed, at),
        canonical(ctx, at, target, IWallet::hopVersionCall {})
    )?;
    if version != U256::ONE {
        return Err(bad(ctx));
    }
    let mut markets = vec![base.market];
    for &e in &entries[1..] {
        let state = lite::shape(e)
            .filter(|s| {
                e.family.id == first.family.id && s.dex == base.dex && s.wrapped == base.wrapped
            })
            .ok_or_else(|| bad(ctx))?;
        if !markets.contains(&state.market) {
            lite::check(ctx, e, at).await?;
        }
        markets.push(state.market);
    }
    let route = super::fluid_lite_hop::Route { markets, path };
    route.physical(base.wrapped).ok_or_else(|| bad(ctx))?;
    Ok((base, route))
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

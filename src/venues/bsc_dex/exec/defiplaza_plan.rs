//! Построение прямой транзакции и шага V5 для конкретного пула и его исходной валюты.
use super::{
    chain::Step,
    defiplaza_wallet::{Proof, Terms},
    plan::{DirectCall, Plan},
};
use crate::{
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::{
        ctx::Ctx,
        pools::{dagrow, defiplaza, direct_module, registry::Entry},
        quote::{v2_wallet_sim as sim, white_state},
        wallet::approve,
    },
};
use alloy_primitives::{Address, Bytes, U256};
use serde_json::Value;

fn bad(ctx: &Ctx) -> crate::gateway::errors::GatewayError {
    ctx.err(
        ErrorCode::NoRoute,
        "DefiPlaza: рынок, валюты или модуль кошелька не совпали",
    )
}
pub(crate) fn graph_target(ctx: &Ctx, e: &Entry) -> Option<Address> {
    let crate::core::types::PoolState::DefiPlaza(s) = &e.pool.state else {
        return None;
    };
    if !matches!(&e.pool.reference,crate::core::types::PoolRef::Multi{contract,token0,token1} if *contract==s.pool && [*token0,*token1]==s.tokens && s.tokens==[e.pool.token0,e.pool.token1])
        || !e.family.variant.defiplaza()
        || ctx.dex_addr("dex.factory", e.family.id).ok()? != s.pool
        || ctx.dex_addr("dex.router", e.family.id).ok()? != s.module
        || s.module.is_zero()
    {
        return None;
    }
    Some(s.module)
}
pub(crate) fn terms(
    ctx: &Ctx,
    e: &Entry,
    input: Address,
    target: Address,
    native: bool,
) -> GatewayResult<Terms> {
    let crate::core::types::PoolState::DefiPlaza(s) = &e.pool.state else {
        return Err(bad(ctx));
    };
    let stable = matches!(
        s.curve,
        crate::core::types::defiplaza::DefiPlazaCurve::Stable { .. }
    );
    let wrapped = ctx.addr("dex.native_wrapped")?;
    let allowed = direct_module::configured(ctx, e.family.id)?;
    if !allowed.contains(&s.module_hash)
        || graph_target(ctx, e) != Some(target)
        || !s.tokens.contains(&input)
        || native && (stable || !s.tokens.contains(&wrapped))
    {
        return Err(bad(ctx));
    }
    Ok(Terms {
        pool: s.pool,
        target,
        wrapped,
        native,
        stable,
        module_hash: s.module_hash,
    })
}
pub(crate) async fn check_module(ctx: &Ctx, e: &Entry, t: Terms, at: &Value) -> GatewayResult<()> {
    let s = Box::pin(defiplaza::state(
        ctx,
        e.family,
        t.pool,
        (e.pool.token0, e.pool.token1),
        at,
    ))
    .await?;
    if s.module != t.target || s.module_hash != t.module_hash {
        return Err(bad(ctx));
    }
    Ok(())
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
        wire: sim::Wire::DefiPlaza(t),
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
    plan.set_direct_market(DirectCall::DefiPlaza(std::sync::Arc::new(proof)));
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

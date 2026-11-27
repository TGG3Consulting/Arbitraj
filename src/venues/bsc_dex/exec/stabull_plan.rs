//! Выбор Curve, официального роутера или адаптера с проверкой роли в каноническом состоянии.
use super::{
    chain::Step,
    plan::{DirectCall, Plan},
    stabull_wallet::{Form, Terms},
};
use crate::{
    gateway::errors::{ErrorCode, GatewayError, GatewayResult},
    venues::bsc_dex::{
        ctx::Ctx,
        node::call,
        pools::{
            dagrow, direct_module, exact_wallet_tokens, registry::Entry, solidly_classic_factory,
            stabull,
        },
        quote::{v2_wallet_sim as sim, white_state},
        wallet::approve,
    },
};
use alloy_primitives::{Address, Bytes, U256};
use serde_json::Value;
pub(crate) fn bad(ctx: &Ctx) -> GatewayError {
    ctx.err(
        ErrorCode::NoRoute,
        "Stabull: изменились валюта, форма заявки или регистрация Curve",
    )
}
pub(crate) fn form(
    ctx: &Ctx,
    e: &Entry,
    target: Address,
    desired: Option<U256>,
) -> GatewayResult<Form> {
    if target.is_zero() {
        return Err(bad(ctx));
    }
    if target == e.pool.reference.address() {
        return Ok(Form::Curve);
    }
    if e.family.executor_kind.is_some()
        && ctx.dex_addr("dex.router", e.family.id).ok() == Some(target)
    {
        return Ok(Form::Module);
    }
    if desired.is_none() && ctx.dex_addr("dex.quoter", e.family.id).ok() == Some(target) {
        return Ok(Form::Router);
    }
    Err(bad(ctx))
}
pub(crate) async fn checked(
    ctx: &Ctx,
    e: &Entry,
    pair: (Address, Address),
    target: Address,
    desired: Option<U256>,
    at: &Value,
) -> GatewayResult<Terms> {
    let g = ctx.settings.generation();
    let f = e.family;
    let pool = e.pool.reference.address();
    if f.variant.euler(){
        let form=super::euler_wallet::checked(ctx,e,pair,target,at).await?;
        return Ok(Terms{form,pool,target,desired,quote:Address::ZERO});
    }
    let form = form(ctx, e, target, desired)?;
    stabull::market(ctx, f, pool, pair, at).await?;
    match form {
        Form::Euler{..}=>return Err(bad(ctx)),
        Form::Module => direct_module::verify_factory(ctx, f, target, at).await?,
        Form::Curve | Form::Router => {
            exact_wallet_tokens::check(ctx, f.id, pair.0, pair.1, at).await?;
            if form == Form::Router {
                let required = ctx
                    .dex_text("dex.quoter_runtime_hash", f.id)?
                    .parse()
                    .map_err(|_| bad(ctx))?;
                solidly_classic_factory::plain(ctx, target, required, at).await?;
                let owner =
                    call::canonical(ctx, at, target, stabull::IStabull::factoryCall {}).await?;
                if owner != ctx.dex_addr("dex.factory", f.id)? {
                    return Err(bad(ctx));
                }
            }
        }
    }
    let quote = if form == Form::Router {
        stabull::assets(ctx, pool, at).await?[1]
    } else {
        Address::ZERO
    };
    if g != ctx.settings.generation() {
        return Err(bad(ctx));
    }
    Ok(Terms {
        form,
        pool,
        quote,
        target,
        desired,
    })
}
pub(super) async fn plan(
    ctx: &Ctx,
    s: &Step,
    payer: Address,
    height: u64,
    selection: Option<(Address, Option<U256>)>,
) -> GatewayResult<Plan> {
    let g = ctx.settings.generation();
    let e = &s.entry;
    let o = &s.order;
    let (target, desired) = selection.ok_or_else(|| bad(ctx))?;
    if !s.from_wallet || !s.to_wallet {
        return Err(bad(ctx));
    }
    let h = white_state::head(ctx, &format!("{height:#x}")).await?;
    let terms = checked(
        ctx,
        e,
        (o.token_in, o.token_out),
        target,
        desired,
        &h.selector(),
    )
    .await?;
    let order = sim::Order {
        payer,
        target,
        input: o.token_in,
        output: o.token_out,
        amount: o.amount_in,
        minimum: o.min_out,
        wire: sim::Wire::Stabull(terms),
    };
    let deadline = U256::from(
        h.timestamp
            .to::<u64>()
            .checked_add(ctx.int("dex.deadline_s")?)
            .ok_or_else(|| bad(ctx))?,
    );
    let proof = terms.proof(&order, o.min_out, deadline);
    sim::measure(ctx, h, order, true).await?;
    let allowance = dagrow::allowance(ctx, o.token_in, payer, target, &h.selector()).await?;
    let approvals = approve::fork_exact(ctx, o.token_in, target, Some(allowance), o.amount_in)?;
    if g != ctx.settings.generation() {
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
    plan.set_direct_market(DirectCall::Stabull(std::sync::Arc::new(proof)));
    Ok(plan)
}
pub(crate) fn probe(
    ctx: &Ctx,
    e: &Entry,
    o: &super::plan::Order,
    payer: Address,
    now: u64,
) -> GatewayResult<(Vec<approve::Approval>, Address, Bytes)> {
    let pool = e.pool.reference.address();
    let target = if e.family.executor_kind.is_some() {
        ctx.dex_addr("dex.router", e.family.id)?
    } else if exact_wallet_tokens::eligible(ctx, e.family.id, o.token_in, o.token_out) {
        if e.family.variant.euler(){ctx.dex_addr("dex.quoter",e.family.id)?}else{pool}
    } else {
        return Err(bad(ctx));
    };
    let t = Terms {
        form: if e.family.variant.euler(){Form::Euler{v2:e.family.variant.euler_v2(),
                    guarded:e.family.executor_kind.is_some(),vaults:[Address::ZERO;2]}}else{form(ctx, e, target, None)?},
        pool,
        target,
        quote: Address::ZERO,
        desired: None,
    };
    let deadline = U256::from(
        now.checked_add(ctx.int("dex.deadline_s")?)
            .ok_or_else(|| bad(ctx))?,
    );
    let p = super::stabull_wallet::Proof {
        terms: t,
        payer,
        input: o.token_in,
        output: o.token_out,
        maximum: o.amount_in,
        minimum: o.min_out,
        deadline,
    };
    Ok((
        approve::fork_exact(ctx, o.token_in, target, None, o.amount_in)?,
        target,
        p.data(),
    ))
}

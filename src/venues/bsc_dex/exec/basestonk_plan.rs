//! Проверка SimpleSwapRouter и сборка защищённой прямой выплаты с тем же PoolId.
use super::{
    basestonk_wallet::{IWallet, Terms},
    chain::Step,
    plan::{DirectCall, Plan},
};
use crate::{
    core::types::PoolRef,
    gateway::errors::{ErrorCode, GatewayError, GatewayResult},
    venues::bsc_dex::{
        ctx::Ctx,
        node::call,
        pools::{basestonk, dagrow, direct_module, key::PoolKey, launch_market, registry::Entry},
        quote::{
            v2_wallet_sim as sim,
            white_state::{self, Head},
        },
        wallet::approve,
    },
};
use alloy_primitives::{Address, U256};
use std::sync::Arc;

fn bad(ctx: &Ctx) -> GatewayError {
    ctx.err(
        ErrorCode::QuoteMismatch,
        "BaseStonk: не подтверждены адрес, условия или выплата прямого обмена",
    )
}
pub(in crate::venues::bsc_dex) fn terms(
    ctx: &Ctx,
    e: &Entry,
    module: Address,
) -> GatewayResult<Terms> {
    let PoolRef::Singleton { manager, id } = e.pool.reference else {
        return Err(bad(ctx));
    };
    let Some(key) = &e.key else {
        return Err(bad(ctx));
    };
    let PoolKey::Uniswap(k) = key else {
        return Err(bad(ctx));
    };
    if key.id() != id {
        return Err(bad(ctx));
    }
    let (router, _) = basestonk::original(ctx, e.family)?;
    Ok(Terms {
        chain: ctx.chain_id().ok_or_else(|| bad(ctx))?,
        module,
        manager,
        router,
        hook: k.hooks,
        tokens: [k.currency0, k.currency1],
        fee: k.fee.try_into().map_err(|_| bad(ctx))?,
        spacing: k.tick_spacing,
    })
}
pub(in crate::venues::bsc_dex) async fn checked(
    ctx: &Ctx,
    e: &Entry,
    h: Head,
    target: Address,
) -> GatewayResult<Terms> {
    let t = terms(ctx, e, target)?;
    let PoolRef::Singleton { manager, id } = e.pool.reference else {
        return Err(bad(ctx));
    };
    let (_, hash) = basestonk::original(ctx, e.family)?;
    let tag = h.selector();
    let ((), (), router, manager_seen, code_hash) = tokio::try_join!(
        direct_module::verify_code(ctx, e.family.id, target, &tag),
        launch_market::check(
            ctx,
            e.family,
            manager,
            id,
            (e.pool.token0, e.pool.token1),
            &tag
        ),
        call::canonical(ctx, &tag, target, IWallet::routerCall {}),
        call::canonical(ctx, &tag, target, IWallet::managerCall {}),
        call::canonical(ctx, &tag, target, IWallet::routerHashCall {})
    )?;
    if (router, manager_seen, code_hash) != (t.router, manager, hash) {
        return Err(bad(ctx));
    }
    Ok(t)
}
pub(super) async fn plan(
    ctx: &Ctx,
    s: &Step,
    payer: Address,
    height: u64,
    target: Option<Address>,
) -> GatewayResult<Plan> {
    let g = ctx.settings.generation();
    let target = target.ok_or_else(|| bad(ctx))?;
    if !s.from_wallet || !s.to_wallet {
        return Err(bad(ctx));
    }
    let h = white_state::head(ctx, &format!("{height:#x}")).await?;
    let t = checked(ctx, &s.entry, h, target).await?;
    let o = &s.order;
    let order = sim::Order {
        payer,
        target,
        input: o.token_in,
        output: o.token_out,
        amount: o.amount_in,
        minimum: o.min_out,
        wire: sim::Wire::BaseStonk(Arc::new(t.clone())),
    };
    let deadline = U256::from(
        h.timestamp
            .to::<u64>()
            .checked_add(ctx.int("dex.deadline_s")?)
            .ok_or_else(|| bad(ctx))?,
    );
    let proof = t.proof(&order, o.min_out, deadline);
    sim::measure(ctx, h, order, true).await?;
    let n = dagrow::allowance(ctx, o.token_in, payer, target, &h.selector()).await?;
    let approvals = approve::fork_exact(ctx, o.token_in, target, Some(n), o.amount_in)?;
    if g != ctx.settings.generation() {
        return Err(bad(ctx));
    }
    let mut p = Plan::new(
        ctx,
        (approvals, target, payer),
        o.token_out,
        vec![],
        o.min_out,
        deadline,
    )?;
    p.set_direct_market(DirectCall::BaseStonk(Arc::new(proof)));
    Ok(p)
}

pub(in crate::venues::bsc_dex) fn probe(
    ctx: &Ctx,
    e: &Entry,
    o: &super::plan::Order,
    payer: Address,
    now: u64,
) -> GatewayResult<(Vec<approve::Approval>, Address, alloy_primitives::Bytes)> {
    let target = ctx.dex_addr("dex.router", e.family.id)?;
    let p = super::basestonk_wallet::Proof {
        terms: terms(ctx, e, target)?,
        payer,
        input: o.token_in,
        output: o.token_out,
        amount: o.amount_in,
        minimum: o.min_out,
        deadline: U256::from(
            now.checked_add(ctx.int("dex.deadline_s")?)
                .ok_or_else(|| bad(ctx))?,
        ),
    };
    Ok((
        approve::fork_exact(ctx, o.token_in, target, None, o.amount_in)?,
        target,
        p.data(),
    ))
}

//! Аттестация, роли и денежная симуляция на родителе единственного допустимого блока.
use super::{
    angstrom_wallet::{IWallet, Terms},
    chain::Step,
    plan::{DirectCall, Plan},
};
use crate::{
    core::types::PoolRef,
    gateway::errors::{ErrorCode, GatewayError, GatewayResult},
    venues::bsc_dex::{
        ctx::Ctx,
        node::call,
        pools::{angstrom, angstrom_feed, dagrow, direct_module, key::PoolKey, registry::Entry},
        quote::{
            v2_wallet_sim as sim,
            white_state::{self, Head},
        },
        wallet::approve,
    },
};
use alloy_primitives::{Address, U256};
use std::sync::Arc;

pub(crate) fn bad(ctx: &Ctx) -> GatewayError {
    ctx.err(
        ErrorCode::NoRoute,
        "Angstrom: нет действующего подтверждения блока, адресов или денежных условий",
    )
}

pub(in crate::venues::bsc_dex) async fn checked(
    ctx: &Ctx,
    e: &Entry,
    h: Head,
    target: Address,
) -> GatewayResult<Terms> {
    let PoolRef::Singleton { manager, id } = e.pool.reference else {
        return Err(bad(ctx));
    };
    let at = h.selector();
    let (key, adapter) = angstrom::market(
        ctx,
        e.family,
        manager,
        id,
        (e.pool.token0, e.pool.token1),
        &at,
    )
    .await?;
    let PoolKey::Uniswap(k) = key else {
        return Err(bad(ctx));
    };
    let ((), a, m, hook) = tokio::try_join!(
        direct_module::verify_code(ctx, e.family.id, target, &at),
        call::canonical(ctx, &at, target, IWallet::adapterCall {}),
        call::canonical(ctx, &at, target, IWallet::managerCall {}),
        call::canonical(ctx, &at, target, IWallet::hookCall {})
    )?;
    if (a, m, hook) != (adapter, manager, k.hooks) {
        return Err(bad(ctx));
    }
    let number = h.number.to::<u64>();
    let attestation = angstrom_feed::for_child(ctx, e.family.id, number).ok_or_else(|| bad(ctx))?;
    Ok(Terms {
        chain: ctx.chain_id().ok_or_else(|| bad(ctx))?,
        module: target,
        adapter,
        manager,
        hook,
        tokens: [k.currency0, k.currency1],
        fee: k.fee.try_into().map_err(|_| bad(ctx))?,
        spacing: k.tick_spacing,
        height: number.checked_add(1).ok_or_else(|| bad(ctx))?,
        attestation,
    })
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
        wire: sim::Wire::Angstrom(Arc::new(t.clone())),
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
    p.set_direct_market(DirectCall::Angstrom(Arc::new(proof)));
    Ok(p)
}

pub(super) fn probe(
    ctx: &Ctx,
    e: &Entry,
    o: &super::plan::Order,
    payer: Address,
    now: u64,
) -> GatewayResult<(Vec<approve::Approval>, Address, alloy_primitives::Bytes)> {
    let PoolRef::Singleton { manager, id } = e.pool.reference else {
        return Err(bad(ctx));
    };
    let target = ctx.dex_addr("dex.router", e.family.id)?;
    let (key, adapter) = angstrom::identity(ctx, e.family, id)?;
    let PoolKey::Uniswap(k) = key else {
        return Err(bad(ctx));
    };
    let height = e.synced_block.checked_add(1).ok_or_else(|| bad(ctx))?;
    let t = Terms {
        chain: ctx.chain_id().ok_or_else(|| bad(ctx))?,
        module: target,
        adapter,
        manager,
        hook: k.hooks,
        tokens: [k.currency0, k.currency1],
        fee: k.fee.try_into().map_err(|_| bad(ctx))?,
        spacing: k.tick_spacing,
        height,
        attestation: angstrom_feed::for_child(ctx, e.family.id, e.synced_block)
            .ok_or_else(|| bad(ctx))?,
    };
    let p = super::angstrom_wallet::Proof {
        terms: t,
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

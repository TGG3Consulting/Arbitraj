//! Устанавливает условия выбранного рынка и проверяет привязку модуля для проб, графа и отправки.
use super::fluid_lite_wallet::{Terms, abi::IWallet};
pub(super) use super::fluid_plan_build::{finish, hop_plan, plan};
use crate::{
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::{
        ctx::Ctx,
        node::call::canonical,
        pools::{direct_module, fluid_dex_lite as lite, registry::Entry},
        quote::v2_wallet_sim as sim,
        wallet::approve,
    },
};
use alloy_primitives::{Address, U256};
use serde_json::Value;
pub(super) fn bad(ctx: &Ctx) -> crate::gateway::errors::GatewayError {
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

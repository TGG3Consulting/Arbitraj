//! Проверка пула launchpad на едином каноническом хеше перед построением лестницы и отправкой.
use super::{
    abi_v4::IStateView,
    discover::Found,
    discover_v4,
    families::Family,
    key::PoolKey,
    launch_abi::{ILaunch, bound},
    launch_keys::{self, invalid},
    nami_batch as batch,
    verify::Candidate,
};
use crate::{gateway::errors::GatewayResult, venues::bsc_dex::ctx::Ctx};
use alloy_primitives::{Address, B256};
use serde_json::Value;

pub(super) struct Market {
    pub key: PoolKey,
    pub asset: Address,
    pub buys: bool,
    pub sells: bool,
}

pub(in crate::venues::bsc_dex) async fn check(
    ctx: &Ctx,
    f: &Family,
    manager: Address,
    id: B256,
    pair: (Address, Address),
    at: &Value,
) -> GatewayResult<()> {
    market(ctx, f, manager, id, pair, at).await.map(|_| ())
}

pub(super) async fn market(
    ctx: &Ctx,
    f: &Family,
    manager: Address,
    id: B256,
    pair: (Address, Address),
    at: &Value,
) -> GatewayResult<Market> {
    let generation = ctx.settings.generation();
    let key = launch_keys::configured(ctx, f, id)?;
    let PoolKey::Uniswap(k) = &key else {
        return Err(invalid(ctx));
    };
    if (k.currency0, k.currency1) != pair
        || key.id() != id
        || ctx.dex_addr("dex.pool_manager", f.id)? != manager
    {
        return Err(invalid(ctx));
    }
    let roles = launch_keys::roles(ctx, f)?;
    let hook = k.hooks;
    let mgr = ILaunch::poolManagerCall {};
    let slot = IStateView::getSlot0Call { poolId: id };
    let view = ctx.dex_addr("dex.state_view", f.id)?;
    let quoter = ctx.dex_addr("dex.quoter", f.id)?;
    let calls = [
        batch::view(at, hook, mgr.clone()),
        batch::view(at, view, mgr.clone()),
        batch::view(at, quoter, mgr.clone()),
        batch::view(at, view, slot.clone()),
    ];
    let ((), [m, v, q, s]) = tokio::try_join!(
        bound(ctx, &roles, "hook", hook, at),
        batch::read(ctx, calls)
    )?;
    if batch::decode(&m, mgr.clone())? != manager
        || batch::decode(&v, mgr.clone())? != manager
        || batch::decode(&q, mgr)? != manager
        || batch::decode(&s, slot)?.sqrtPriceX96.is_zero()
    {
        return Err(invalid(ctx));
    }
    let (asset, buys, sells) = super::launch_terms::read(ctx, f, &key, &roles, at).await?;
    if generation != ctx.settings.generation() {
        return Err(invalid(ctx));
    }
    Ok(Market {
        key,
        asset,
        buys,
        sells,
    })
}

pub(super) async fn verify(ctx: &Ctx, c: &Candidate) -> GatewayResult<Found> {
    let id = c.pool.parse().map_err(|_| invalid(ctx))?;
    let manager = ctx.dex_addr("dex.pool_manager", c.family.id)?;
    let at = super::nami::block(ctx, "latest").await?;
    let pair = super::discover::sorted(c.coin, c.quote);
    let market = market(ctx, c.family, manager, id, pair, &at).await?;
    let PoolKey::Uniswap(k) = &market.key else {
        return Err(invalid(ctx));
    };
    let spacing = k.tick_spacing.as_i32();
    Ok(discover_v4::from_key(
        c.family,
        manager,
        id,
        market.key,
        Some(spacing),
        None,
    ))
}

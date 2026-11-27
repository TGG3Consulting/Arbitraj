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

pub(super) fn currencies(ctx: &Ctx, mut found: Found) -> GatewayResult<Found> {
    if found.family.variant == super::family_variant::Variant::LaunchO1Wrapped {
        let wrapped = ctx.addr("dex.native_wrapped")?;
        if !found.token0.is_zero() || found.token1 == wrapped || wrapped.is_zero() {
            return Err(invalid(ctx));
        }
        found.token0 = wrapped;
        found.coin = wrapped;
    }
    Ok(found)
}

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
    let wrapped_mode = f.variant == super::family_variant::Variant::LaunchO1Wrapped;
    let currency0 = if wrapped_mode {
        ctx.addr("dex.native_wrapped")?
    } else {
        k.currency0
    };
    if super::discover::sorted(currency0, k.currency1) != super::discover::sorted(pair.0, pair.1)
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
    let hook_manager = async {
        if f.variant != super::family_variant::Variant::LaunchBaseline {
            let [raw] = batch::read(ctx, [batch::view(at, hook, mgr.clone())]).await?;
            if batch::decode(&raw, mgr.clone())? != manager {
                return Err(invalid(ctx));
            }
        }
        // У Mercury фактический PoolKey проверяется по реестру Relay и Initialize.
        Ok(())
    };
    let calls = [
        batch::view(at, view, mgr.clone()),
        batch::view(at, quoter, mgr.clone()),
        batch::view(at, view, slot.clone()),
    ];
    let ((), (), [v, q, s]) = tokio::try_join!(
        bound(ctx, &roles, "hook", hook, at),
        hook_manager,
        batch::read(ctx, calls)
    )?;
    if batch::decode(&v, mgr.clone())? != manager
        || batch::decode(&q, mgr)? != manager
        || batch::decode(&s, slot)?.sqrtPriceX96.is_zero()
    {
        return Err(invalid(ctx));
    }
    let (asset, buys, sells) = super::launch_terms::read(ctx, f, &key, &roles, at).await?;
    if wrapped_mode {
        if !k.currency0.is_zero() || asset != k.currency1 || asset == currency0 {
            return Err(invalid(ctx));
        }
        let module = ctx.dex_addr("dex.router", f.id)?;
        let module_manager = ILaunch::managerCall {};
        let module_wrapped = ILaunch::wrappedCall {};
        let market_key = ILaunch::marketCall { id, token: asset };
        let views = [
            batch::view(at, module, module_manager.clone()),
            batch::view(at, module, module_wrapped.clone()),
            batch::view(at, module, market_key.clone()),
        ];
        let ((), [m, w, k]) = tokio::try_join!(
            super::direct_module::verify_code(ctx, f.id, module, at),
            batch::read(ctx, views)
        )?;
        if batch::decode(&m, module_manager)? != manager
            || batch::decode(&w, module_wrapped)? != currency0
            || !super::launch_abi::same(&batch::decode(&k, market_key)?, &key)
        {
            return Err(invalid(ctx));
        }
    }
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
    currencies(
        ctx,
        discover_v4::from_key(c.family, manager, id, market.key, Some(spacing), None),
    )
}

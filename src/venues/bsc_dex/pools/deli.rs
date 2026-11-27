//! Deli constant-product использует собственные резервы и обычный ввод ERC-20
//! в Uniswap V4. PoolId восстанавливается из ключа Initialize даже без позиции в PositionManager.
use super::{
    abi_v4::IStateView,
    deli_keys::{bindings, configured},
    discover::Found,
    discover_v4,
    families::Family,
    key::PoolKey,
    nami, nami_batch as batch,
    verify::Candidate,
};
use crate::gateway::errors::{ErrorCode, GatewayResult};
use crate::venues::bsc_dex::ctx::Ctx;
use alloy_primitives::{Address, B256, U256, keccak256};
use serde_json::{Value, json};

alloy::sol! {
    interface IDeli {
        function poolManager() external view returns(address);
        function poolInitialized(bytes32 id) external view returns(bool);
        function getReserves(bytes32 id) external view returns(uint128 r0, uint128 r1);
        function feeProcessor() external view returns(address);
        function incentiveGauge() external view returns(address);
        function dailyEpochGauge() external view returns(address);
    }
}

pub(super) fn invalid(ctx: &Ctx) -> crate::gateway::errors::GatewayError {
    ctx.err(
        ErrorCode::QuoteMismatch,
        "Deli: ключ, роли или код хука расходятся с проверяемым рынком",
    )
}

pub(in crate::venues::bsc_dex) async fn market(
    ctx: &Ctx,
    f: &Family,
    manager: Address,
    id: B256,
    pair: (Address, Address),
    at: &Value,
) -> GatewayResult<(PoolKey, U256, U256)> {
    let generation = ctx.settings.generation();
    let pk = configured(ctx, f, id)?;
    let PoolKey::Uniswap(k) = &pk else {
        return Err(invalid(ctx));
    };
    if (k.currency0, k.currency1) != pair {
        return Err(invalid(ctx));
    }
    let enabled = IDeli::poolInitializedCall { id };
    let reserves = IDeli::getReservesCall { id };
    let calls = [
        batch::view(at, k.hooks, enabled.clone()),
        batch::view(at, k.hooks, reserves.clone()),
    ];
    let ((), [e, r]) = tokio::try_join!(
        checked_roles(ctx, f, manager, k.hooks, id, at),
        batch::read(ctx, calls)
    )?;
    if ctx.settings.generation() != generation || !batch::decode(&e, enabled)? {
        return Err(invalid(ctx));
    }
    let state = batch::decode(&r, reserves)?;
    Ok((pk, U256::from(state.r0), U256::from(state.r1)))
}

pub(super) async fn checked_roles(
    ctx: &Ctx,
    f: &Family,
    manager: Address,
    hook: Address,
    id: B256,
    at: &Value,
) -> GatewayResult<()> {
    let generation = ctx.settings.generation();
    let roles = bindings(ctx, f)?;
    if roles[0].0 != hook || ctx.dex_addr("dex.pool_manager", f.id)? != manager {
        return Err(invalid(ctx));
    }
    let view = ctx.dex_addr("dex.state_view", f.id)?;
    let mgr = IDeli::poolManagerCall {};
    let slot = IStateView::getSlot0Call { poolId: id };
    let processor = IDeli::feeProcessorCall {};
    let incentive = IDeli::incentiveGaugeCall {};
    let daily = IDeli::dailyEpochGaugeCall {};
    let requests = [
        batch::view(at, hook, mgr.clone()),
        batch::view(at, view, slot.clone()),
        batch::view(at, view, mgr.clone()),
        batch::view(at, hook, processor.clone()),
        batch::view(at, hook, incentive.clone()),
        batch::view(at, hook, daily.clone()),
    ];
    let code_calls = roles.map(|(a, _)| ("eth_getCode", json!([a, at])));
    let ([m, s, vm, p, i, d], codes) =
        tokio::try_join!(batch::read(ctx, requests), batch::read(ctx, code_calls))?;
    if generation != ctx.settings.generation()
        || batch::decode(&m, mgr.clone())? != manager
        || batch::decode(&vm, mgr)? != manager
        || batch::decode(&s, slot)?.sqrtPriceX96.is_zero()
        || batch::decode(&p, processor)? != roles[1].0
        || batch::decode(&i, incentive)? != roles[2].0
        || batch::decode(&d, daily)? != roles[3].0
        || !codes
            .iter()
            .zip(roles)
            .all(|(b, (_, hash))| !b.is_empty() && keccak256(b) == hash)
    {
        return Err(invalid(ctx));
    }
    Ok(())
}

pub(super) async fn verify(ctx: &Ctx, c: &Candidate) -> GatewayResult<Found> {
    let id = c.pool.parse().map_err(|_| invalid(ctx))?;
    let manager = ctx.dex_addr("dex.pool_manager", c.family.id)?;
    let at = nami::block(ctx, "latest").await?;
    let pair = super::discover::sorted(c.coin, c.quote);
    let (pk, _, _) = market(ctx, c.family, manager, id, pair, &at).await?;
    let PoolKey::Uniswap(k) = &pk else {
        return Err(invalid(ctx));
    };
    let spacing = k.tick_spacing.as_i32();
    Ok(discover_v4::from_key(
        c.family,
        manager,
        id,
        pk,
        Some(spacing),
        None,
    ))
}

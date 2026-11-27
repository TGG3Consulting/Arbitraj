//! Реестр Baseline Mercury связывает PoolId, bToken, валюту резерва и обе паузы.
use super::{
    baseline_components::{IMercury, components},
    families::Family,
    key::PoolKey,
    launch_abi::{ILaunch, same},
    launch_keys::{Role, invalid},
    nami_batch as batch,
};
use crate::{gateway::errors::GatewayResult, venues::bsc_dex::ctx::Ctx};
use alloy_primitives::Address;
use serde_json::Value;
pub(super) async fn read(
    ctx: &Ctx,
    f: &Family,
    key: &PoolKey,
    roles: &[Role],
    at: &Value,
) -> GatewayResult<(Address, bool, bool)> {
    let PoolKey::Uniswap(k) = key else {
        return Err(invalid(ctx));
    };
    let hook = k.hooks;
    if hook != ctx.dex_addr("dex.factory", f.id)? || k.fee != 0 || k.tick_spacing.as_i32() != 1 {
        return Err(invalid(ctx));
    }
    let lookup = IMercury::poolIdToBTokenCall { id: key.id() };
    let ((), [raw]) = tokio::try_join!(
        components(ctx, hook, roles, at),
        batch::read(ctx, [batch::view(at, hook, lookup.clone())])
    )?;
    let token = batch::decode(&raw, lookup)?;
    if token.is_zero() || (token != k.currency0 && token != k.currency1) {
        return Err(invalid(ctx));
    }
    let pk = ILaunch::poolKeyCall { token };
    let reserve = IMercury::reserveCall { token };
    let has = IMercury::hasHookCall { token };
    let paused = IMercury::isPoolPausedCall { token };
    let global = IMercury::isProtocolPausedCall {};
    let [p, r, h, s, g] = batch::read(
        ctx,
        [
            batch::view(at, hook, pk.clone()),
            batch::view(at, hook, reserve.clone()),
            batch::view(at, hook, has.clone()),
            batch::view(at, hook, paused.clone()),
            batch::view(at, hook, global.clone()),
        ],
    )
    .await?;
    if !same(&batch::decode(&p, pk)?, key)
        || batch::decode(&r, reserve)?
            != if token == k.currency0 {
                k.currency1
            } else {
                k.currency0
            }
        || !batch::decode(&h, has)?
    {
        return Err(invalid(ctx));
    }
    let live = !batch::decode(&s, paused)? && !batch::decode(&g, global)?;
    Ok((token, live, live))
}

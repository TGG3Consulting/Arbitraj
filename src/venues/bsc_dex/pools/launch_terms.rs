//! Условия допуска отдельного направления для каждого подтверждённого семейства хука.
use super::{
    families::Family,
    family_variant::Variant,
    key::PoolKey,
    launch_abi::{ILaunch, bound, same},
    launch_keys::{Role, invalid},
    nami_batch as batch,
};
use crate::{gateway::errors::GatewayResult, venues::bsc_dex::ctx::Ctx};
use alloy::sol_types::SolCall;
use alloy_primitives::{Address, U256};
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
    let pair = (k.currency0, k.currency1);
    let hook = k.hooks;
    let id = key.id();
    Ok(match f.variant {
        Variant::LaunchDoppler => {
            let wrapped = ctx.addr("dex.native_wrapped")?;
            let asset = if pair.0 == wrapped {
                pair.1
            } else if pair.1 == wrapped {
                pair.0
            } else {
                return Err(invalid(ctx));
            };
            let state = ILaunch::getStateCall { asset };
            let air = ILaunch::airlockCall {};
            let [r, a] = batch::read(
                ctx,
                [
                    batch::view(at, hook, state.clone()),
                    batch::view(at, hook, air.clone()),
                ],
            )
            .await?;
            let r = batch::decode(&r, state)?;
            if r.status != 2 || r.numeraire != wrapped || !same(&r.poolKey, key) {
                return Err(invalid(ctx));
            }
            let flags = ILaunch::isDopplerHookEnabledCall { child: r.child };
            let (_, _, [fl]) = tokio::try_join!(
                bound(ctx, roles, "after_hook", r.child, at),
                bound(ctx, roles, "airlock", batch::decode(&a, air)?, at),
                batch::read(ctx, [batch::view(at, hook, flags.clone())])
            )?;
            if batch::decode(&fl, flags)? != U256::from(3) {
                return Err(invalid(ctx));
            }
            (asset, true, true)
        }
        Variant::LaunchUmia => return super::umia::read(ctx, f, key, roles, at).await,
        Variant::LaunchBaseline => return super::baseline::read(ctx,f,key,roles,at).await,
        Variant::LaunchIndex => {
            let vault = ILaunch::vaultOfCall { id };
            let factory = ILaunch::factoryCall {};
            let registry = ILaunch::registryCall {};
            let [v, a, r] = batch::read(
                ctx,
                [
                    batch::view(at, hook, vault.clone()),
                    batch::view(at, hook, factory.clone()),
                    batch::view(at, hook, registry.clone()),
                ],
            )
            .await?;
            let asset = batch::decode(&v, vault)?;
            let factory = batch::decode(&a, factory)?;
            if asset != pair.0 && asset != pair.1 {
                return Err(invalid(ctx));
            }
            let is = ILaunch::isVaultCall { vault: asset };
            let pools = ILaunch::poolsOfCall { vault: asset };
            let pass = ILaunch::passRequiredCall { vault: asset };
            let buy = ILaunch::buysPausedCall { vault: asset };
            let sell = ILaunch::sellsPausedCall {};
            let calls = [
                batch::view(at, factory, is.clone()),
                batch::view(at, hook, pools.clone()),
                batch::view(at, hook, pass.clone()),
                batch::view(at, hook, buy.clone()),
                batch::view(at, hook, sell.clone()),
            ];
            let (_, _, [i, p, g, b, s]) = tokio::try_join!(
                bound(ctx, roles, "factory", factory, at),
                bound(ctx, roles, "registry", batch::decode(&r, registry)?, at),
                batch::read(ctx, calls)
            )?;
            let keys = batch::decode(&p, pools)?;
            if !batch::decode(&i, is)? || !(same(&keys.stableKey, key) || same(&keys.wethKey, key))
            {
                return Err(invalid(ctx));
            }
            (
                asset,
                !batch::decode(&g, pass)? && !batch::decode(&b, buy)?,
                !batch::decode(&s, sell)?,
            )
        }
        Variant::LaunchO1 | Variant::LaunchO1Wrapped => {
            let config = ILaunch::poolConfigCall { id };
            let factory = ILaunch::factoryCall {};
            let escrow = ILaunch::feeEscrowCall {};
            let [r, a, e] = batch::read(
                ctx,
                [
                    batch::view(at, hook, config),
                    batch::view(at, hook, factory.clone()),
                    batch::view(at, hook, escrow.clone()),
                ],
            )
            .await?;
            if !matches!(r.len(), 256 | 352) {
                return Err(invalid(ctx));
            }
            let cfg = ILaunch::poolConfigCall::abi_decode_returns_validate(&r[..64])
                .map_err(|_| invalid(ctx))?;
            if !cfg.initialized {
                return Err(invalid(ctx));
            }
            tokio::try_join!(
                bound(ctx, roles, "factory", batch::decode(&a, factory)?, at),
                bound(ctx, roles, "escrow", batch::decode(&e, escrow)?, at)
            )?;
            (
                if cfg.tokenIsCurrency0 { pair.0 } else { pair.1 },
                true,
                true,
            )
        }
        _ => return Err(invalid(ctx)),
    })
}

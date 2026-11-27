//! Связывает физический спотовый PoolId с зарегистрированным vault и валютами venture Umia.
//! Все роли читаются на том же каноническом хеше, что и состояние PoolManager.
use super::{
    families::Family,
    key::PoolKey,
    launch_abi::{ILaunch, bound, same},
    launch_keys::{Role, invalid},
    nami_batch as batch,
};
use crate::{gateway::errors::GatewayResult, venues::bsc_dex::ctx::Ctx};
use alloy::sol_types::SolCall;
use alloy_primitives::Address;
use serde_json::Value;

alloy::sol! {
    interface IUmia {
        function pools(bytes32 id) external view returns(address launcher, address venture, address vault);
        function ventureLiquidityVault(address venture) external view returns(address);
        function isLBP(address launcher) external view returns(bool);
        function token() external view returns(address);
        function moneyToken() external view returns(address);
        function ventureToken() external view returns(address);
        function venture() external view returns(address);
        function hook() external view returns(address);
        function hub() external view returns(address);
        function spotMigrator() external view returns(address);
        function activeManager() external view returns(address);
    }
}

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
    let hub = ctx.dex_addr("dex.factory", f.id)?;
    let manager = ctx.dex_addr("dex.pool_manager", f.id)?;
    let pool = IUmia::poolsCall { id: key.id() };
    let factory = ILaunch::factoryCall {};
    let ((), [p, fac]) = tokio::try_join!(
        bound(ctx, roles, "registry", hub, at),
        batch::read(
            ctx,
            [
                batch::view(at, k.hooks, pool),
                batch::view(at, k.hooks, factory.clone())
            ]
        )
    )?;
    if !matches!(p.len(), 96 | 128) {
        return Err(invalid(ctx));
    }
    let r = IUmia::poolsCall::abi_decode_returns_validate(&p[..96]).map_err(|_| invalid(ctx))?;
    if [r.venture, r.vault, r.launcher].contains(&Address::ZERO) {
        return Err(invalid(ctx));
    }
    let registrar = batch::decode(&fac, factory)?;
    let registered = IUmia::isLBPCall {
        launcher: r.launcher,
    };
    let current = IUmia::ventureLiquidityVaultCall { venture: r.venture };
    let token = IUmia::tokenCall {};
    let cash = IUmia::moneyTokenCall {};
    let venture = IUmia::ventureCall {};
    let asset = IUmia::ventureTokenCall {};
    let hook = IUmia::hookCall {};
    let pm = ILaunch::poolManagerCall {};
    let key_call = ILaunch::getPoolKeyCall {};
    let calls = [
        batch::view(at, registrar, registered.clone()),
        batch::view(at, hub, current.clone()),
        batch::view(at, r.venture, token.clone()),
        batch::view(at, r.venture, cash.clone()),
        batch::view(at, r.vault, venture.clone()),
        batch::view(at, r.vault, asset.clone()),
        batch::view(at, r.vault, cash.clone()),
        batch::view(at, r.vault, hook.clone()),
        batch::view(at, r.vault, pm.clone()),
        batch::view(at, r.vault, key_call.clone()),
    ];
    let ((), [is, cur, t, m, v, vt, vm, h, man, raw_key]) = tokio::try_join!(
        bound(ctx, roles, "factory", registrar, at),
        batch::read(ctx, calls)
    )?;
    let venture_token = batch::decode(&t, token)?;
    let money_token = batch::decode(&m, cash.clone())?;
    let current_vault = batch::decode(&cur, current)?;
    if current_vault != r.vault {
        // Предыдущий vault после миграции может сохранить активную ликвидность.
        // Его runtime подтверждён настройкой по событию Hub с обоими адресами.
        let check = IUmia::ventureCall {};
        let ((), [linked]) = tokio::try_join!(
            bound(ctx, roles, "component", r.vault, at),
            batch::read(ctx, [batch::view(at, current_vault, check.clone())])
        )?;
        if batch::decode(&linked, check)? != r.venture {
            return Err(invalid(ctx));
        }
    }
    if batch::decode(&v, venture)? != r.venture
        || batch::decode(&vt, asset)? != venture_token
        || batch::decode(&vm, cash)? != money_token
        || super::discover::sorted(venture_token, money_token) != (k.currency0, k.currency1)
        || batch::decode(&h, hook)? != k.hooks
        || batch::decode(&man, pm)? != manager
        || !same(&batch::decode(&raw_key, key_call)?, key)
    {
        return Err(invalid(ctx));
    }
    let from_launchpad = batch::decode(&is, registered)?;
    if p.len() == 128 {
        let active = IUmia::activeManagerCall {};
        let h = IUmia::hubCall {};
        let m = IUmia::spotMigratorCall {};
        let [a, owner, mig] = batch::read(
            ctx,
            [
                batch::view(at, r.vault, active.clone()),
                batch::view(at, k.hooks, h.clone()),
                batch::view(at, hub, m.clone()),
            ],
        )
        .await?;
        let operator = batch::decode(&a, active)?;
        if operator.is_zero()
            || p[96..108] != [0u8; 12]
            || Address::from_slice(&p[108..]) != operator
            || batch::decode(&owner, h)? != hub
            || (!from_launchpad && batch::decode(&mig, m)? != r.launcher)
        {
            return Err(invalid(ctx));
        }
    } else if !from_launchpad {
        return Err(invalid(ctx));
    }
    Ok((venture_token, true, true))
}

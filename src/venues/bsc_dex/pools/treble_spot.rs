//! Сверяет исходную фабрику Treble Spot, текущий плагин пары и связанный с ним виртуальный пул фермы.
use super::{
    abi_algebra_integral::IAlgebraIntegralPool as Pool, families::Family, nami_batch as batch,
    registry::Entry, solidly_classic_factory::plain,
};
use crate::{
    gateway::errors::{ErrorCode, GatewayError, GatewayResult},
    venues::bsc_dex::ctx::Ctx,
};
use alloy::sol;
use alloy_primitives::{Address, B256};
use serde_json::Value;

sol! {
    interface Role {
        function factory() external view returns(address);
        function poolDeployer() external view returns(address);
        function token0() external view returns(address);
        function token1() external view returns(address);
        function plugin() external view returns(address);
        function pluginByPool(address market) external view returns(address);
        function pool() external view returns(address);
        function poolByPair(address x,address y) external view returns(address);
        function customPoolByPair(address deployer,address x,address y) external view returns(address);
        function getCurrentFee() external view returns(uint16);
        function getSecurityRegistry() external view returns(address);
        function getPoolStatus(address p) external view returns(uint8);
        function algebraFactory() external view returns(address);
        function limitOrderManager() external view returns(address);
        function basePluginFactory() external view returns(address);
        function incentive() external view returns(address);
        function farmingAddress() external view returns(address);
        function farmingCenter() external view returns(address);
        function eternalFarming() external view returns(address);
        function algebraPoolDeployer() external view returns(address);
        function virtualPoolAddresses(address market) external view returns(address);
    }
}
macro_rules! views {
    ($ctx:expr, $tag:expr; $( $var:ident: $addr:expr => $call:expr ),+) => {
        let [$($var),+] = batch::read($ctx,[$(batch::view($tag,$addr,$call)),+]).await?;
        $(let $var=batch::decode(&$var,$call)?;)+
    }
}
fn bad(ctx: &Ctx) -> GatewayError {
    ctx.err(
        ErrorCode::NotApplicable,
        "Treble Spot: регистрация, комиссия или внешний участник отличаются от проверенной формы",
    )
}

pub(super) async fn fee(
    ctx: &Ctx,
    f: &Family,
    pool: Address,
    at: &Value,
    roles: (Address, Address),
    hashes: &[B256],
) -> GatewayResult<u16> {
    let (factory, registry) = roles;
    let [
        registry_hash,
        limit_hash,
        security_hash,
        farm_hash,
        center_hash,
    ] = hashes
    else {
        return Err(bad(ctx));
    };
    plain(ctx, registry, *registry_hash, at).await?;
    views!(ctx,at; owner:pool=>Role::factoryCall{}, x:pool=>Role::token0Call{}, y:pool=>Role::token1Call{},
        plugin:pool=>Role::pluginCall{}, origin:registry=>Role::algebraFactoryCall{},
        registered:registry=>Role::pluginByPoolCall{market:pool},physical:factory=>Role::poolDeployerCall{},
        state:pool=>Pool::globalStateCall{});
    if owner != factory
        || x.is_zero()
        || x >= y
        || origin != factory
        || registered != plugin
        || plugin.is_zero()
        || physical != ctx.dex_addr("dex.algebra_pool_deployer", f.id)?
        || !state.unlocked
        || state.pluginConfig != super::base_plugin_v1::flags(f.variant)
    {
        return Err(bad(ctx));
    }
    let deployer = super::supernova::role(ctx, f.id, physical, pool, x, y)?;
    let pair = if deployer.is_zero() {
        views!(ctx,at; listed:factory=>Role::poolByPairCall{x,y});
        listed
    } else {
        views!(ctx,at; listed:factory=>Role::customPoolByPairCall{deployer,x,y});
        listed
    };
    views!(ctx,at; market:plugin=>Role::poolCall{},live:pool=>Pool::feeCall{},projected:plugin=>Role::getCurrentFeeCall{},
        limit:plugin=>Role::limitOrderManagerCall{},security:plugin=>Role::getSecurityRegistryCall{},virtual_pool:plugin=>Role::incentiveCall{});
    if pair != pool || market != pool || live != projected {
        return Err(bad(ctx));
    }
    if !limit.is_zero() {
        plain(ctx, limit, *limit_hash, at).await?;
        views!(ctx,at; origin:limit=>Role::factoryCall{},deployer:limit=>Role::poolDeployerCall{},creator:limit=>Role::basePluginFactoryCall{});
        if (origin, deployer, creator) != (factory, physical, registry) {
            return Err(bad(ctx));
        }
    }
    if !security.is_zero() {
        plain(ctx, security, *security_hash, at).await?;
        views!(ctx,at; origin:security=>Role::algebraFactoryCall{},status:security=>Role::getPoolStatusCall{p:pool});
        if origin != factory || status != 0 {
            return Err(bad(ctx));
        }
    }
    if !virtual_pool.is_zero() {
        views!(ctx,at; farm:virtual_pool=>Role::farmingAddressCall{},attached:virtual_pool=>Role::pluginCall{});
        if attached != plugin {
            return Err(bad(ctx));
        }
        plain(ctx, farm, *farm_hash, at).await?;
        views!(ctx,at; center:farm=>Role::farmingCenterCall{});
        plain(ctx, center, *center_hash, at).await?;
        views!(ctx,at; back:center=>Role::eternalFarmingCall{},vp:center=>Role::virtualPoolAddressesCall{market:pool},
            origin:center=>Role::algebraPoolDeployerCall{});
        if (back, vp, origin) != (farm, virtual_pool, physical) {
            return Err(bad(ctx));
        }
    }
    Ok(live)
}

pub(super) async fn router(ctx: &Ctx, e: &Entry, target: Address, at: &Value) -> GatewayResult<()> {
    let fee = super::base_plugin_v1::check(ctx, e.family, e.pool.reference.address(), at).await?;
    views!(ctx,at; owner:target=>Role::factoryCall{},deployer:target=>Role::poolDeployerCall{});
    if owner != ctx.dex_addr("dex.factory", e.family.id)?
        || deployer != ctx.dex_addr("dex.algebra_pool_deployer", e.family.id)?
        || Some(u32::from(fee)) != e.pool.fee_pips
    {
        return Err(bad(ctx));
    }
    Ok(())
}

#[cfg(test)]
#[path = "treble_spot_profile_tests.rs"]
mod profile_tests;

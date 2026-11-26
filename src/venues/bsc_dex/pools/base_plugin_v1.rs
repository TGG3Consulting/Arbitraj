//! Algebra Integral BasePluginV1. Его фабрика создаёт обычный плагин для каждой пары и хранит привязку.
//! Флаг BEFORE_SWAP вызывает обновление оракула и ставки. При активной внешней программе требуется отдельная проверка.
use super::{
    abi_algebra_integral::IAlgebraIntegralPool as Pool, families::Family, family_variant::Variant,
    nami_batch as batch,
};
use crate::{
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::ctx::Ctx,
};
use alloy::sol;
use alloy_primitives::{Address, B256};
use serde_json::Value;

sol! {
    interface Binding {
        function factory() external view returns(address);
        function poolDeployer() external view returns(address);
        function token0() external view returns(address);
        function token1() external view returns(address);
        function defaultPluginFactory() external view returns(address);
        function algebraFactory() external view returns(address);
        function poolByPair(address a,address b) external view returns(address);
        function pluginByPool(address market) external view returns(address);
        function plugin() external view returns(address);
        function pool() external view returns(address);
        function getCurrentFee() external view returns(uint16);
        function rebalanceManager() external view returns(address);
        function limitOrderManager() external view returns(address);
        function getSecurityRegistry() external view returns(address);
        function getPoolStatus(address market) external view returns(uint8);
        function basePluginFactory() external view returns(address);
        function incentive() external view returns(address);
    }
}

pub(super) fn flags(variant: Variant) -> u8 {
    if variant == Variant::TrebleIntegral {
        0xd7
    } else {
        0xc1
    }
}

macro_rules! views {
    ($ctx:expr, $tag:expr; $( $var:ident: $addr:expr => $call:expr ),+) => {
        let [$($var),+] = batch::read($ctx,[$(batch::view($tag,$addr,$call)),+]).await?;
        $(let $var=batch::decode(&$var,$call)?;)+
    }
}

pub(in crate::venues::bsc_dex) async fn check(
    ctx: &Ctx,
    f: &Family,
    pool: Address,
    tag: &Value,
) -> GatewayResult<u16> {
    let generation = ctx.settings.generation();
    let bad = || {
        ctx.err(ErrorCode::NotApplicable,"Algebra BasePluginV1: регистрация, код фабрики плагина, флаги или источник комиссии изменились")
    };
    let factory = super::solidly_classic_factory::at(ctx, f, tag).await?;
    let registry = ctx.dex_addr("dex.plugin_factory", f.id)?;
    let hashes = ctx.dex_list("dex.plugin_runtime_hashes", f.id)?;
    let hashes = hashes
        .iter()
        .map(|h| h.parse::<B256>().map_err(|_| bad()))
        .collect::<GatewayResult<Vec<_>>>()?;
    let (expected, treble) = match (f.variant, hashes.as_slice()) {
        (Variant::BasePluginV1, [factory]) => (*factory, None),
        (Variant::TrebleIntegral, [factory, limit, security])
            if limit != security && !limit.is_zero() && !security.is_zero() =>
        {
            (*factory, Some((*limit, *security)))
        }
        _ => return Err(bad()),
    };
    super::solidly_classic_factory::plain(ctx, registry, expected, tag).await?;
    let templates = super::verify_algebra_dir::hashes_for(ctx, f.id).map_err(|_| bad())?;
    views!(ctx,tag; owner:pool=>Binding::factoryCall{}, current:factory=>Binding::defaultPluginFactoryCall{},
        back:registry=>Binding::algebraFactoryCall{}, deployer:factory=>Binding::poolDeployerCall{},
        token0:pool=>Binding::token0Call{}, token1:pool=>Binding::token1Call{},
        plugin:pool=>Binding::pluginCall{}, listed:registry=>Binding::pluginByPoolCall{market:pool},
        state:pool=>Pool::globalStateCall{});
    if owner != factory
        || back != factory
        || current != registry
        || plugin.is_zero()
        || listed != plugin
        || !state.unlocked
        || state.pluginConfig != flags(f.variant)
        || token0.is_zero()
        || token0 >= token1
        || !templates.iter().any(|hash| {
            super::verify_algebra_dir::pool_address(deployer, token0, token1, *hash) == pool
        })
    {
        return Err(bad());
    }
    views!(ctx,tag; paired:factory=>Binding::poolByPairCall{a:token0,b:token1}, market:plugin=>Binding::poolCall{},
        incentive:plugin=>Binding::incentiveCall{}, projected:plugin=>Binding::getCurrentFeeCall{}, actual:pool=>Pool::feeCall{});
    if paired != pool || market != pool || !incentive.is_zero() || projected != actual {
        return Err(bad());
    }
    if let Some((limit_hash, security_hash)) = treble {
        views!(ctx,tag; rebalance:plugin=>Binding::rebalanceManagerCall{}, limit:plugin=>Binding::limitOrderManagerCall{},
            security:plugin=>Binding::getSecurityRegistryCall{});
        if !rebalance.is_zero() {
            return Err(bad());
        }
        if !limit.is_zero() {
            super::solidly_classic_factory::plain(ctx, limit, limit_hash, tag).await?;
            views!(ctx,tag; origin:limit=>Binding::factoryCall{}, creator:limit=>Binding::poolDeployerCall{},
                base:limit=>Binding::basePluginFactoryCall{});
            if origin != factory || creator != deployer || base != registry {
                return Err(bad());
            }
        }
        if !security.is_zero() {
            super::solidly_classic_factory::plain(ctx, security, security_hash, tag).await?;
            views!(ctx,tag; origin:security=>Binding::algebraFactoryCall{}, status:security=>Binding::getPoolStatusCall{market:pool});
            if origin != factory || status != 0 {
                return Err(bad());
            }
        }
    }
    if generation != ctx.settings.generation() {
        return Err(ctx.err(
            ErrorCode::StaleData,
            "Настройки Algebra изменились во время чтения",
        ));
    }
    Ok(actual)
}

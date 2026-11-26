//! Algebra Integral BasePluginV1. Его фабрика создаёт обычный плагин для каждой пары и хранит привязку.
//! Флаг BEFORE_SWAP вызывает обновление оракула и ставки. При активной внешней программе требуется отдельная проверка.
use super::{
    abi_algebra_integral::IAlgebraIntegralPool as Pool, families::Family, nami_batch as batch,
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
        function incentive() external view returns(address);
    }
}

pub(super) const FLAGS: u8 = 0xc1;

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
    let [expected] = hashes.as_slice() else {
        return Err(bad());
    };
    let expected = expected.parse::<B256>().map_err(|_| bad())?;
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
        || state.pluginConfig != FLAGS
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
    if generation != ctx.settings.generation() {
        return Err(ctx.err(
            ErrorCode::StaleData,
            "Настройки Algebra изменились во время чтения",
        ));
    }
    Ok(actual)
}

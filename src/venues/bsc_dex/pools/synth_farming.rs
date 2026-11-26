//! Algebra V1.9 с единой ставкой и отдельным EternalVirtualPool для расчёта фермерских наград.
use super::{families::Family, family_variant::Variant, nami_batch as batch};
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
        function token0() external view returns(address);
        function token1() external view returns(address);
        function farmingAddress() external view returns(address);
        function activeIncentive() external view returns(address);
        function poolDeployer() external view returns(address);
        function poolByPair(address a,address b) external view returns(address);
        function virtualPoolAddresses(address pool) external view returns(address limitVP,address eternalVP);
        function eternalFarming() external view returns(address);
        function farmingCenter() external view returns(address);
        function farmingCenterAddress() external view returns(address);
        function pool() external view returns(address);
    }
}

pub(super) fn step(ctx: &Ctx, f: &Family) -> GatewayResult<Option<u32>> {
    if f.variant != Variant::AlgebraSingleV19 {
        return Ok(None);
    }
    let n = ctx.dex_int("dex.fusion_next_block_s", f.id)?;
    u32::try_from(n)
        .ok()
        .filter(|x| *x > 0 && *x < super::super::math::fee_algebra_v1::WINDOW)
        .map(Some)
        .ok_or_else(|| {
            ctx.err(
                ErrorCode::NotConfigured,
                "Algebra V1.9: некорректный интервал блоков",
            )
        })
}
macro_rules! reads {
    ($ctx:expr,$tag:expr; $( $var:ident: $addr:expr => $call:expr ),+) => {
        let [$($var),+]=batch::read($ctx,[$(batch::view($tag,$addr,$call)),+]).await?;
        $(let $var=batch::decode(&$var,$call)?;)+
    }
}

pub(in crate::venues::bsc_dex) async fn check(
    ctx: &Ctx,
    f: &Family,
    pool: Address,
    tag: &Value,
) -> GatewayResult<()> {
    let generation = ctx.settings.generation();
    let reject = || {
        ctx.err(
            ErrorCode::NotApplicable,
            "Algebra V1.9: происхождение пула или действующей фермы не подтверждено",
        )
    };
    let factory = super::solidly_classic_factory::at(ctx, f, tag).await?;
    let template = super::verify_algebra_dir::hashes_for(ctx, f.id).map_err(|_| reject())?;
    reads!(ctx,tag; owner:pool=>Binding::factoryCall{},a:pool=>Binding::token0Call{},b:pool=>Binding::token1Call{},
        deployer:factory=>Binding::poolDeployerCall{},active:pool=>Binding::activeIncentiveCall{},center:factory=>Binding::farmingAddressCall{});
    if owner != factory
        || a.is_zero()
        || a >= b
        || !template
            .iter()
            .any(|h| super::verify_algebra_dir::pool_address(deployer, a, b, *h) == pool)
    {
        return Err(reject());
    }
    reads!(ctx,tag; verified:factory=>Binding::poolByPairCall{a,b});
    if verified != pool {
        return Err(reject());
    }
    if !active.is_zero() {
        let hashes = ctx.dex_list("dex.plugin_runtime_hashes", f.id)?;
        let [farming, registry] = hashes.as_slice() else {
            return Err(reject());
        };
        let registry = registry.parse::<B256>().map_err(|_| reject())?;
        let farming = farming.parse::<B256>().map_err(|_| reject())?;
        super::solidly_classic_factory::plain(ctx, center, registry, tag).await?;
        reads!(ctx,tag; farm:center=>Binding::eternalFarmingCall{},registered:center=>Binding::virtualPoolAddressesCall{pool},
            actual_pool:active=>Binding::poolCall{},creator:active=>Binding::farmingAddressCall{}, actual_center:active=>Binding::farmingCenterAddressCall{});
        if !registered.limitVP.is_zero()
            || registered.eternalVP != active
            || actual_pool != pool
            || creator != farm
            || actual_center != center
        {
            return Err(reject());
        }
        super::solidly_classic_factory::plain(ctx, farm, farming, tag).await?;
        reads!(ctx,tag; back:farm=>Binding::farmingCenterCall{});
        if back != center {
            return Err(reject());
        }
    }
    if generation != ctx.settings.generation() {
        return Err(ctx.err(
            ErrorCode::StaleData,
            "Настройки Algebra изменились при чтении",
        ));
    }
    Ok(())
}

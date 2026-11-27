//! Привязка официальных HelloV3Factory, HelloV3PoolDeployer и квотера к одному рынку.
//! При подключении нового lmPool обратные вызовы требуют отдельной приёмки.
use super::{families::Family, nami_batch as batch};
use crate::{
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::ctx::Ctx,
};
use alloy::sol;
use alloy_primitives::Address;
use serde_json::Value;

sol! {
    interface IHello {
        function poolDeployer() external view returns (address);
        function deployer() external view returns (address);
        function factoryAddress() external view returns (address);
        function protocolFeeRecipient() external view returns (address);
        function lmPool() external view returns (address);
    }
}

pub(super) async fn creator(
    ctx: &Ctx,
    family: &Family,
    factory: Address,
    quoter: Address,
    pool: Address,
    at: &Value,
) -> GatewayResult<Address> {
    let declared = IHello::poolDeployerCall {};
    let bound = IHello::deployerCall {};
    let lm = IHello::lmPoolCall {};
    let [origin, source, hook] = batch::read(
        ctx,
        [
            batch::view(at, factory, declared.clone()),
            batch::view(at, quoter, bound.clone()),
            batch::view(at, pool, lm.clone()),
        ],
    )
    .await?;
    let deployer = batch::decode(&origin, declared)?;
    let hook = batch::decode(&hook, lm)?;
    if family.variant == super::family_variant::Variant::VvsV3 && !hook.is_zero() {
        super::vvs_rewards::check(ctx,family.id,pool,hook,at).await?;
    }
    if deployer.is_zero()
        || batch::decode(&source, bound)? != deployer
        || (family.variant == super::family_variant::Variant::HelloV3 && !hook.is_zero())
        || (family.variant == super::family_variant::Variant::HelloV3 && ctx.dex_text("dex.v3_swap_callback", family.id)?
            != "pancakeV3SwapCallback(int256,int256,bytes)")
    {
        return Err(ctx.err(
            ErrorCode::NotApplicable,
            "V3: не подтверждён создатель пула, callback или модуль наград",
        ));
    }
    let fa = IHello::factoryAddressCall {};
    let [linked] = batch::read(ctx, [batch::view(at, deployer, fa.clone())]).await?;
    if batch::decode(&linked, fa)? != factory {
        return Err(ctx.err(
            ErrorCode::NotApplicable,
            "V3: создатель связан с другой фабрикой",
        ));
    }
    Ok(deployer)
}

/// Перевод накопленной платы протокола может совпасть с адресом участника этого маршрута.
pub(crate) async fn separate_fee(
    ctx: &Ctx,
    family: &Family,
    path: &[&super::registry::Entry],
    at: &Value,
) -> GatewayResult<()> {
    let factory = ctx.dex_addr("dex.factory", family.id)?;
    let call = IHello::protocolFeeRecipientCall {};
    let [raw] = batch::read(ctx, [batch::view(at, factory, call.clone())]).await?;
    let treasury = batch::decode(&raw, call)?;
    if !treasury.is_zero()
        && (treasury == crate::venues::bsc_dex::wallet::signer::address(ctx)?
            || Some(treasury) == crate::venues::bsc_dex::exec::executor::address(ctx)?
            || path.iter().any(|e| e.pool.reference.address() == treasury))
    {
        return Err(ctx.err(ErrorCode::NoRoute,"V3: получатель сборов участвует в маршруте, чистая выплата отдельно не подтверждена"));
    }
    Ok(())
}

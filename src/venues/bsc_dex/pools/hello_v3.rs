//! Привязка официальных HelloV3Factory, HelloV3PoolDeployer и квотера к одному рынку.
//! При подключении нового lmPool обратные вызовы требуют отдельной приёмки.
use super::{families::Family, nami_batch as batch};
use crate::{gateway::errors::{ErrorCode, GatewayResult}, venues::bsc_dex::ctx::Ctx};
use alloy::sol;
use alloy_primitives::Address;
use serde_json::Value;

sol! {
    interface IHello {
        function poolDeployer() external view returns (address);
        function deployer() external view returns (address);
        function factoryAddress() external view returns (address);
        function lmPool() external view returns (address);
    }
}

pub(super) async fn creator(ctx:&Ctx, family:&Family, factory:Address, quoter:Address, pool:Address, at:&Value)->GatewayResult<Address> {
    let declared=IHello::poolDeployerCall {};
    let bound=IHello::deployerCall {};
    let lm=IHello::lmPoolCall {};
    let [origin,source,hook]=batch::read(ctx,[
        batch::view(at,factory,declared.clone()),batch::view(at,quoter,bound.clone()),batch::view(at,pool,lm.clone()),
    ]).await?;
    let deployer=batch::decode(&origin,declared)?;
    if deployer.is_zero() || batch::decode(&source,bound)?!=deployer || !batch::decode(&hook,lm)?.is_zero()
        || ctx.dex_text("dex.v3_swap_callback",family.id)?!="pancakeV3SwapCallback(int256,int256,bytes)" {
        return Err(ctx.err(ErrorCode::NotApplicable,"HELLO V3: не подтверждён создатель пула, callback или модуль наград"));
    }
    let fa=IHello::factoryAddressCall {};
    let [linked]=batch::read(ctx,[batch::view(at,deployer,fa.clone())]).await?;
    if batch::decode(&linked,fa)?!=factory {
        return Err(ctx.err(ErrorCode::NotApplicable,"HELLO V3: создатель связан с другой фабрикой"));
    }
    Ok(deployer)
}

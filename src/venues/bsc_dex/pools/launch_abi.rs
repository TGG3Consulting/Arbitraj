//! Проверенные ABI и адресные привязки контрактов launchpad.
use super::{
    key::PoolKey,
    launch_keys::{Role, invalid},
    nami_batch as batch,
};
use crate::{gateway::errors::GatewayResult, venues::bsc_dex::ctx::Ctx};
use alloy_primitives::{Address, keccak256};
use serde_json::{Value, json};

alloy::sol! {
    interface ILaunch {
        struct Key {address currency0;address currency1;uint24 fee;int24 tickSpacing;address hooks;}
        function manager() external view returns(address);
        function wrapped() external view returns(address);
        function market(bytes32 id,address token) external view returns(Key);
        function poolManager() external view returns(address);
        function factory() external view returns(address);
        function registry() external view returns(address);
        function feeEscrow() external view returns(address);
        function airlock() external view returns(address);
        function poolConfig(bytes32 id) external view returns(bool initialized,bool tokenIsCurrency0);
        function vaultOf(bytes32 id) external view returns(address);
        function isVault(address vault) external view returns(bool);
        function poolsOf(address vault) external view returns(Key stableKey,Key wethKey);
        function passRequired(address vault) external view returns(bool);
        function buysPaused(address vault) external view returns(bool);
        function sellsPaused() external view returns(bool);
        function getState(address asset) external view returns(address numeraire,uint256 total,address child,bytes data,uint8 status,Key poolKey,int24 farTick);
        function isDopplerHookEnabled(address child) external view returns(uint256);
    }
}

pub(super) fn same(key: &ILaunch::Key, pk: &PoolKey) -> bool {
    matches!(pk,PoolKey::Uniswap(k) if (key.currency0,key.currency1,key.fee.to::<u32>(),key.tickSpacing,key.hooks)==(k.currency0,k.currency1,k.fee,k.tick_spacing,k.hooks))
}

pub(super) async fn bound(
    ctx: &Ctx,
    roles: &[Role],
    name: &str,
    a: Address,
    at: &Value,
) -> GatewayResult<()> {
    let role = roles
        .iter()
        .find(|r| r.kind == name && r.address == a)
        .ok_or_else(|| invalid(ctx))?;
    let [code] = batch::read(ctx, [("eth_getCode", json!([a, at]))]).await?;
    if code.is_empty() || keccak256(code) != role.hash {
        return Err(invalid(ctx));
    }
    Ok(())
}

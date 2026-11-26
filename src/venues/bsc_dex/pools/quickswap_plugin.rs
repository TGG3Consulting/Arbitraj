//! Реестр, байткод прокси плагина и реализация Beacon QuickSwap V4 Base.
use super::super::{
    ctx::Ctx,
    node::{call, rpc},
};
use crate::gateway::errors::{ErrorCode, GatewayResult};
use alloy::sol;
use alloy::sol_types::SolCall;
use alloy_primitives::{Address, B256, Bytes, b256, keccak256};
use serde_json::json;

sol! {
    interface IQuickPlugin {
        function plugin() external view returns (address);
        function defaultPluginFactory() external view returns (address);
        function pluginByPool(address pool) external view returns (address);
        function implementation() external view returns (address);
    }
}

const BEACON_SLOT: B256 = b256!("a3f0ad74e5423aebfd80d3ef4346578335a9a72aeaee59ff6cb3582b35133d50");
const PROXY_TEMPLATE: B256 =
    b256!("b98e9ecd492a849a01a727b796337558e32dd5ae038cecadc494f9b0f919b08a");

/// Верифицированный AlgebraPluginProxy содержит ровно одну immutable-подстановку адреса пула.
pub fn matches_proxy(code: &[u8], pool: Address) -> bool {
    if code.get(75..107) != Some(pool.into_word().as_slice()) {
        return false;
    }
    let mut normalized = code.to_vec();
    normalized[75..107].fill(0);
    keccak256(normalized) == PROXY_TEMPLATE
}

async fn at<C: SolCall>(
    ctx: &Ctx,
    to: Address,
    method: C,
    tag: &serde_json::Value,
) -> GatewayResult<C::Return> {
    let input = Bytes::from(method.abi_encode());
    let out: Bytes = rpc::call(ctx, "eth_call", json!([{"to":to,"input":input},tag])).await?;
    call::decode::<C>(&out)
}

pub async fn check(ctx: &Ctx, id: &str, pool: Address, tag: &str) -> GatewayResult<()> {
    #[derive(serde::Deserialize)]
    struct Header {
        hash: B256,
    }
    let head: Header = rpc::call(ctx, "eth_getBlockByNumber", json!([tag, false])).await?;
    let canonical = json!({"blockHash":head.hash, "requireCanonical":true});
    let factory = ctx.dex_addr("dex.factory", id)?;
    let registry = ctx.dex_addr("dex.plugin_factory", id)?;
    let beacon = ctx.dex_addr("dex.plugin_beacon", id)?;
    let expected = ctx.dex_addr("dex.plugin_implementation", id)?;
    let selected = IQuickPlugin::pluginByPoolCall { pool };
    let (plugin, active, listed, actual) = tokio::try_join!(
        at(ctx, pool, IQuickPlugin::pluginCall {}, &canonical),
        at(
            ctx,
            factory,
            IQuickPlugin::defaultPluginFactoryCall {},
            &canonical
        ),
        at(ctx, registry, selected, &canonical),
        at(ctx, beacon, IQuickPlugin::implementationCall {}, &canonical)
    )?;
    if ctx.chain_id() != Some(8453) || active != registry || plugin != listed || actual != expected
    {
        return Err(ctx.err(
            ErrorCode::InvalidAddress,
            "QuickSwap: изменился реестр, плагин пула или реализация beacon",
        ));
    }
    let (code, word): (Bytes, B256) = tokio::try_join!(
        rpc::call(ctx, "eth_getCode", json!([plugin, canonical])),
        rpc::call(
            ctx,
            "eth_getStorageAt",
            json!([plugin, BEACON_SLOT, canonical])
        )
    )?;
    if Address::from_word(word) != beacon || !matches_proxy(&code, pool) {
        return Err(ctx.err(
            ErrorCode::InvalidAddress,
            "QuickSwap: прокси плагина, его immutable pool или beacon расходятся с проверенными",
        ));
    }
    Ok(())
}

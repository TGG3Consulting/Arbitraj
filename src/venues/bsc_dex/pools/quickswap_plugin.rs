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
/// Сверяет immutable пула и хеш нормализованного байткода AlgebraPluginProxy.
pub(super) fn proxy_matches(code: &[u8], pool: Address, offset: usize, expected: B256) -> bool {
    let Some(end) = offset.checked_add(32) else {
        return false;
    };
    if code.get(offset..end) != Some(pool.into_word().as_slice()) {
        return false;
    }
    let mut normalized = code.to_vec();
    normalized[offset..end].fill(0);
    keccak256(normalized) == expected
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
    let generation = ctx.settings.generation();
    let binding_error = || {
        ctx.err(
            ErrorCode::NotApplicable,
            "QuickSwap: нужны один хеш шаблона proxy и смещение immutable pool",
        )
    };
    let hashes = ctx.dex_list("dex.plugin_runtime_hashes", id)?;
    let [template] = hashes.as_slice() else {
        return Err(binding_error());
    };
    let template: B256 = template.trim().parse().map_err(|_| binding_error())?;
    let offset =
        usize::try_from(ctx.dex_int("dex.plugin_pool_offset", id)?).map_err(|_| binding_error())?;
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
    if Address::from_word(word) != beacon || !proxy_matches(&code, pool, offset, template) {
        return Err(ctx.err(
            ErrorCode::InvalidAddress,
            "QuickSwap: прокси плагина, его immutable pool или beacon расходятся с проверенными",
        ));
    }
    if generation != ctx.settings.generation() {
        return Err(ctx.err(
            ErrorCode::StaleData,
            "QuickSwap: настройки изменились во время сверки",
        ));
    }
    Ok(())
}

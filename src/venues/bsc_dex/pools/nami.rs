//! Algebra Integral Nami. Проверяет пути фабрики, beacon и плагина и действующий режим комиссии в одном состоянии.
use super::super::ctx::Ctx;
use super::{abi_algebra::IAlgebraFactory, elfomo, integral_block::At};
use crate::gateway::errors::{ErrorCode, GatewayResult};
use alloy::{sol, sol_types::SolCall};
use alloy_primitives::{Address, B256, Bytes, U256, keccak256};
use serde_json::{Value, json};

pub(super) const PLUGIN_FLAGS: u8 = 0xc3; // BEFORE_SWAP | AFTER_SWAP | AFTER_INIT | DYNAMIC_FEE (Plugins.sol).

sol! {
    interface INami {
        function factory() external view returns(address);
        function token0() external view returns(address);
        function token1() external view returns(address);
        function defaultPluginFactory() external view returns(address);
        function pluginByPool(address pool) external view returns(address);
        function plugin() external view returns(address);
        function pool() external view returns(address);
        function pluginFactory() external view returns(address);
        function implementation() external view returns(address);
        function extension() external view returns(address);
        function getMevxExecutor() external view returns(address);
        function feeConfig() external view returns(uint16 alpha1,uint16 alpha2,uint32 beta1,uint32 beta2,uint16 gamma1,uint16 gamma2,uint16 baseFee);
        function feeSource() external view returns(address source,bool enabled);
        function feeBounds() external view returns(uint16 floor,uint16 cap,uint16 premium0For1,uint16 premium1For0,uint32 decayWindow,uint16 maxDiscountBps);
        function swapFee(bool zeroToOne) external view returns(uint16);
        function quoteExactInputSingle(address from,address to,address deployer,uint256 amount,uint160 limit) external returns(uint256 out,uint16 fee);
    }
}

fn proxy_matches(code: &[u8], pool: Address, offset: usize, expected: B256) -> bool {
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

pub(in crate::venues::bsc_dex) async fn block(ctx: &Ctx, tag: &str) -> GatewayResult<Value> {
    elfomo::block(ctx, tag).await
}

macro_rules! views {
    ($ctx:expr,$tag:expr; $( $name:ident : $target:expr => $call:expr ),+ $(,)?) => {
        let [$($name),+]=super::nami_batch::read($ctx,[$(super::nami_batch::view($tag,$target,$call)),+]).await?;
        $(let $name=super::nami_batch::decode(&$name,$call)?;)+
    }
}

pub async fn check(ctx: &Ctx, id: &str, pool: Address, tag: &Value) -> GatewayResult<u16> {
    let generation = ctx.settings.generation();
    let wrong = || {
        ctx.err(ErrorCode::NotApplicable,"Nami: топология, код, режим комиссии или условия отправителя расходятся с проверенным ABI")
    };
    let addr = |key| ctx.dex_addr(key, id);
    let (factory, quoter, registry, beacon, implementation, extension) = (
        addr("dex.factory")?,
        addr("dex.quoter")?,
        addr("dex.plugin_factory")?,
        addr("dex.plugin_beacon")?,
        addr("dex.plugin_implementation")?,
        addr("dex.plugin_extension")?,
    );
    let hashes: Vec<B256> = ctx
        .dex_list("dex.plugin_runtime_hashes", id)?
        .iter()
        .map(|s| s.trim().parse().ok())
        .collect::<Option<_>>()
        .ok_or_else(wrong)?;
    let [template, imp_hash, ext_hash] = hashes.as_slice() else {
        return Err(wrong());
    };
    let offset =
        usize::try_from(ctx.dex_int("dex.plugin_pool_offset", id)?).map_err(|_| wrong())?;
    views!(ctx,tag;
        plugin:pool=>INami::pluginCall{},linked:factory=>INami::defaultPluginFactoryCall{},
        list:registry=>INami::pluginByPoolCall{pool},actual:beacon=>INami::implementationCall{},
        from_factory:pool=>INami::factoryCall{},quoter_factory:quoter=>INami::factoryCall{},
        t0:pool=>INami::token0Call{},t1:pool=>INami::token1Call{},
        gs:pool=>super::abi_algebra_integral::IAlgebraIntegralPool::globalStateCall{});
    if !gs.unlocked
        || gs.pluginConfig != PLUGIN_FLAGS
        || plugin.is_zero()
        || (linked, list, actual, from_factory, quoter_factory)
            != (registry, plugin, implementation, factory, factory)
    {
        return Err(wrong());
    }
    let beacon_slot = U256::from_be_bytes(keccak256(b"eip1967.proxy.beacon").0) - U256::ONE;
    let [word, proxy_code, imp_code, ext_code] = super::nami_batch::read(
        ctx,
        [
            (
                "eth_getStorageAt",
                json!([plugin, format!("{beacon_slot:#066x}"), tag]),
            ),
            ("eth_getCode", json!([plugin, tag])),
            ("eth_getCode", json!([implementation, tag])),
            ("eth_getCode", json!([extension, tag])),
        ],
    )
    .await?;
    if word.as_ref() != beacon.into_word().as_slice()
        || !proxy_matches(&proxy_code, pool, offset, *template)
        || keccak256(&imp_code) != *imp_hash
        || keccak256(&ext_code) != *ext_hash
        || imp_code.is_empty()
        || ext_code.is_empty()
    {
        return Err(wrong());
    }
    views!(ctx,tag;
        actual_pool:plugin=>INami::poolCall{},listed_pool:factory=>IAlgebraFactory::poolByPairCall{tokenA:t0,tokenB:t1},
        actual_reg:plugin=>INami::pluginFactoryCall{},ext:plugin=>INami::extensionCall{},
        discounted:plugin=>INami::getMevxExecutorCall{},cfg:plugin=>INami::feeConfigCall{},
        source:plugin=>INami::feeSourceCall{},bounds:plugin=>INami::feeBoundsCall{},
        zt:plugin=>INami::swapFeeCall{zeroToOne:true},ot:plugin=>INami::swapFeeCall{zeroToOne:false});
    if (actual_pool, listed_pool, actual_reg, ext) != (pool, pool, registry, extension)
        || ctx.executor()? == Some(discounted)
        || quoter == discounted
        || cfg.alpha1 != 0
        || cfg.alpha2 != 0
        || source.enabled
        || bounds.cap != 0
        || zt == 0
        || zt != ot
        || zt != cfg.baseFee
        || generation != ctx.settings.generation()
    {
        return Err(wrong());
    }
    Ok(zt)
}

pub async fn quote(
    ctx: &Ctx,
    id: &str,
    from: Address,
    to: Address,
    amount: U256,
    block: &str,
) -> GatewayResult<U256> {
    let hash = elfomo::block(ctx, block).await?;
    let call = single(from, to, amount);
    let response = At::Canonical(&hash)
        .view(ctx, ctx.dex_addr("dex.quoter", id)?, &call)
        .await?;
    Ok(response.out)
}

fn single(from: Address, to: Address, amount: U256) -> INami::quoteExactInputSingleCall {
    INami::quoteExactInputSingleCall {
        from,
        to,
        deployer: Address::ZERO,
        amount,
        limit: Default::default(),
    }
}

pub fn calldata(from: Address, to: Address, amount: U256) -> Bytes {
    single(from, to, amount).abi_encode().into()
}

pub fn decoded(raw: &[u8]) -> Option<U256> {
    Some(
        INami::quoteExactInputSingleCall::abi_decode_returns_validate(raw)
            .ok()?
            .out,
    )
}

/// Один учтённый шаг совместим с Nami. При нескольких шагах внешний callback может изменить чужой пул.
pub(in crate::venues::bsc_dex) fn path_ok<'a>(
    entries: impl IntoIterator<Item = &'a super::registry::Entry>,
) -> bool {
    let mut n = 0;
    let mut external = false;
    for e in entries {
        n += 1;
        external |= e.family.variant.external_algebra_swap();
        if external && n > 1 {
            return false;
        }
    }
    true
}

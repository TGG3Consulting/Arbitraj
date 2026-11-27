//! Подлинность фабрики и плагина Cypher Algebra. Ставки обеих сторон берутся из официальной котировки,
//! включая customDeployer и адрес действительного отправителя. https://docs.cyphereth.com/docs/contracts/mainnet/
use super::{families::Family, nami_batch as batch, solidly_classic_factory as code};
use crate::{
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::{ctx::Ctx, node::rpc},
};
use alloy_primitives::{Address, B256, Bytes, keccak256};
use serde_json::{Value, json};
alloy::sol! {interface Cypher{
    function factory() external view returns(address);
    function poolDeployer() external view returns(address);
    function token0() external view returns(address);
    function token1() external view returns(address);
    function poolByPair(address a,address b) external view returns(address);
    function customPoolByPair(address deployer,address a,address b) external view returns(address);
    function plugin() external view returns(address);
    function globalState() external view returns(uint160 price,int24 tick,uint16 lastFee,uint8 flags,uint16 community,bool unlocked);
}}
fn template(code: &[u8], pool: Address) -> B256 {
    let mut out = code.to_vec();
    let mut i = 0;
    while i < out.len() {
        let size = if (0x60..=0x7f).contains(&out[i]) {
            usize::from(out[i] - 0x5f)
        } else {
            0
        };
        if let Some(data) = out.get_mut(i + 1..i + 1 + size)
            && ((size == 20 && data == pool.as_slice())
                || (size == 32 && data == pool.into_word().as_slice()))
        {
            data.fill(0);
        }
        i += size + 1;
    }
    keccak256(out)
}
pub(in crate::venues::bsc_dex) async fn checked(
    ctx: &Ctx,
    f: &Family,
    pool: Address,
    (a, b): (Address, Address),
    at: &Value,
) -> GatewayResult<Address> {
    let generation = ctx.settings.generation();
    let bad = || {
        ctx.err(
            ErrorCode::NotApplicable,
            "Cypher: изменились код, реестр или состояние плагина",
        )
    };
    let factory = ctx.dex_addr("dex.factory", f.id)?;
    let q = ctx.dex_addr("dex.quoter", f.id)?;
    let (reads, (), ()) = tokio::try_join!(
        batch::read(
            ctx,
            [
                batch::view(at, factory, Cypher::poolDeployerCall {}),
                batch::view(at, q, Cypher::poolDeployerCall {}),
                batch::view(at, q, Cypher::factoryCall {}),
                batch::view(at, pool, Cypher::factoryCall {}),
                batch::view(at, pool, Cypher::token0Call {}),
                batch::view(at, pool, Cypher::token1Call {}),
                batch::view(at, pool, Cypher::pluginCall {}),
                batch::view(at, pool, Cypher::globalStateCall {})
            ]
        ),
        code::plain(
            ctx,
            factory,
            super::safeswap_trade::hash(ctx, f.id, "dex.factory_runtime_hash")?,
            at
        ),
        code::plain(
            ctx,
            q,
            super::safeswap_trade::hash(ctx, f.id, "dex.quoter_runtime_hash")?,
            at
        )
    )?;
    let address = |i: usize| batch::decode(&reads[i], Cypher::factoryCall {});
    let physical = address(0)?;
    if physical != ctx.dex_addr("dex.algebra_pool_deployer", f.id)?
        || physical.is_zero()
        || physical != address(1)?
        || address(2)? != factory
        || address(3)? != factory
        || address(4)? != a
        || address(5)? != b
        || a.is_zero()
        || a >= b
    {
        return Err(bad());
    }
    let custom = super::supernova::role(ctx, f.id, physical, pool, a, b)?;
    let registered = if custom.is_zero() {
        batch::view(at, factory, Cypher::poolByPairCall { a, b })
    } else {
        batch::view(
            at,
            factory,
            Cypher::customPoolByPairCall {
                deployer: custom,
                a,
                b,
            },
        )
    };
    let plugin = address(6)?;
    let live = batch::decode(&reads[7], Cypher::globalStateCall {})?;
    if !live.unlocked || live.price.is_zero() || (plugin.is_zero() && live.flags != 0) {
        return Err(bad());
    }
    let check_plugin = async {
        if !plugin.is_zero() {
            let bytecode: Bytes = rpc::call(ctx, "eth_getCode", json!([plugin, at])).await?;
            if bytecode.is_empty() {
                return Err(bad());
            }
            if f.variant == super::family_variant::Variant::TheoremIntegral {
                super::theorem::plugin(ctx,f,pool,plugin,at).await?;
            }
            let hash = template(&bytecode, pool);
            if !ctx
                .dex_list("dex.plugin_runtime_hashes", f.id)?
                .iter()
                .any(|h| h.parse::<B256>().ok() == Some(hash))
            {
                return Err(bad());
            }
        }
        Ok(())
    };
    let ([listing], ()) = tokio::try_join!(batch::read(ctx, [registered]), check_plugin)?;
    if batch::decode(&listing, Cypher::factoryCall {})? != pool
        || ctx.settings.generation() != generation
    {
        return Err(bad());
    }
    Ok(custom)
}

pub(in crate::venues::bsc_dex) fn deployment(
    ctx: &Ctx,
    e: &super::registry::Entry,
) -> GatewayResult<Address> {
    super::supernova::role(
        ctx,
        e.family.id,
        ctx.dex_addr("dex.algebra_pool_deployer", e.family.id)?,
        e.pool.reference.address(),
        e.pool.token0,
        e.pool.token1,
    )
}
pub(in crate::venues::bsc_dex) async fn router(
    ctx: &Ctx,
    e: &super::registry::Entry,
    router: Address,
    at: &Value,
) -> GatewayResult<()> {
    let p = &e.pool;
    let (_, [record]) = tokio::try_join!(
        checked(
            ctx,
            e.family,
            p.reference.address(),
            (p.token0, p.token1),
            at
        ),
        batch::read(ctx, [batch::view(at, router, Cypher::poolDeployerCall {})])
    )?;
    if batch::decode(&record, Cypher::poolDeployerCall {})?
        != ctx.dex_addr("dex.algebra_pool_deployer", e.family.id)?
    {
        return Err(ctx.err(ErrorCode::NotApplicable, "Cypher: у роутера другой деплоер"));
    }
    Ok(())
}

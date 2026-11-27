//! Реестр DEGENSwap и неизменяемые роли защищённого плательщика на одном блоке Ethereum.
//! https://etherscan.io/address/0x4bf3e2287d4ced7796bfab364c0401dfce4a4f7f#code
use super::{
    abi_check::IPoolImmutables as Pool,
    abi_v2v3::IV2Factory as Factory,
    families::Family,
    nami_batch as batch,
    safeswap_trade::{ISafeDeployment as Roles, hash},
    solidly_classic_factory as code,
};
use crate::{
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::{ctx::Ctx, wallet::signer},
};
use alloy_primitives::{Address, B256};
use serde_json::Value;
alloy::sol! { interface IDegen {
    function routerAddress() external view returns(address);
    function factoryAddress() external view returns(address);
    function getReserves() external view returns(uint112 r0,uint112 r1,uint32 timestamp,address base);
    function adminFee() external view returns(uint256);
    function pairFeeAddress(address pair) external view returns(address);
    function getTotalFee() external view returns(uint256);
    function getAmountsOut(uint256 amount,address[] calldata path) external view returns(uint256[]);
    function balanceOf(address owner) external view returns(uint256);
}}
pub(in crate::venues::bsc_dex) async fn market(
    ctx: &Ctx,
    f: &Family,
    pool: Address,
    (a, b): (Address, Address),
    at: &Value,
) -> GatewayResult<Address> {
    let bad = || {
        ctx.err(
            ErrorCode::NotApplicable,
            "DEGENSwap: не совпали сеть, код, пара или плательщик",
        )
    };
    let generation = ctx.settings.generation();
    let factory = ctx.dex_addr("dex.factory", f.id)?;
    let router = ctx.dex_addr("dex.quoter", f.id)?;
    let module = ctx.dex_addr("dex.router", f.id)?;
    let wrapped = ctx.addr("dex.native_wrapped")?;
    let init = ctx.dex_list("dex.pool_code_hashes", f.id)?;
    let [init] = init.as_slice() else {
        return Err(bad());
    };
    let init = init.parse::<B256>().map_err(|_| bad())?;
    if ctx.chain_id() != Some(1)
        || a.is_zero()
        || a >= b
        || init.is_zero()
        || super::verify::v2_create2(factory, a, b, init) != pool
    {
        return Err(bad());
    }
    let calls = [
        batch::view(
            at,
            factory,
            Factory::getPairCall {
                tokenA: a,
                tokenB: b,
            },
        ),
        batch::view(at, pool, Pool::token0Call {}),
        batch::view(at, pool, Pool::token1Call {}),
        batch::view(at, pool, Pool::factoryCall {}),
        batch::view(at, pool, IDegen::routerAddressCall {}),
        batch::view(at, router, Roles::factoryCall {}),
        batch::view(at, router, IDegen::factoryAddressCall {}),
        batch::view(at, router, Roles::WETHCall {}),
        batch::view(at, module, Roles::factoryCall {}),
        batch::view(at, module, Roles::routerCall {}),
        batch::view(at, module, Roles::tradeCall {}),
        batch::view(at, module, Roles::wrappedCall {}),
        batch::view(at, module, Roles::payerCall {}),
        batch::view(at, module, Roles::executorCall {}),
    ];
    let (rows, (), (), (), ()) = tokio::try_join!(
        batch::read(ctx, calls),
        code::plain(
            ctx,
            factory,
            hash(ctx, f.id, "dex.factory_runtime_hash")?,
            at
        ),
        code::plain(ctx, router, hash(ctx, f.id, "dex.quoter_runtime_hash")?, at),
        code::plain(ctx, pool, hash(ctx, f.id, "dex.pair_runtime_hash")?, at),
        super::direct_module::verify_code(ctx, f.id, module, at)
    )?;
    let required = [
        pool,
        a,
        b,
        factory,
        router,
        factory,
        factory,
        wrapped,
        factory,
        router,
        Address::ZERO,
        wrapped,
        signer::address(ctx)?,
    ];
    for (raw, wanted) in rows.iter().zip(required) {
        if batch::decode(raw, Roles::factoryCall {})? != wanted {
            return Err(bad());
        }
    }
    if ctx
        .executor()?
        .is_some_and(|e| batch::decode(&rows[13], Roles::executorCall {}).ok() != Some(e))
        || generation != ctx.settings.generation()
    {
        return Err(bad());
    }
    Ok(module)
}

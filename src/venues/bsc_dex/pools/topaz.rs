//! Комиссия Topaz CL зависит от tx.origin и откатившегося либо успешного вызова swapFeeModule.
use super::{
    abi_slipstream::{ICLFactory, ICLPool},
    families::Family,
    nami_batch as batch, solidly_classic_factory as binding,
};
use crate::{
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::{ctx::Ctx, node::rpc, wallet::signer},
};
use alloy::sol_types::SolCall;
use alloy_primitives::{Address, B256, Bytes};
use serde_json::{Value, json};
alloy::sol! { interface ITopaz{
    function poolImplementation() external view returns(address);
    function swapFeeModule() external view returns(address);
    function factory() external view returns(address);
}}
pub(crate) fn sender(ctx: &Ctx) -> Address {
    signer::address(ctx).unwrap_or_default()
}

pub(crate) async fn fee(
    ctx: &Ctx,
    f: &Family,
    pool: Address,
    pair: (Address, Address),
    spacing: i32,
    at: &Value,
) -> GatewayResult<u32> {
    let wrong = || {
        ctx.err(
            ErrorCode::NotApplicable,
            "Topaz CL: изменились рынок, модуль платы или размещение квотера",
        )
    };
    let rev = ctx.settings.generation();
    let factory = ctx.dex_addr("dex.factory", f.id)?;
    let quoter = ctx.dex_addr("dex.quoter", f.id)?;
    let module = ctx.dex_addr("dex.cl_fee_module", f.id)?;
    let hash = |key: &str| {
        ctx.dex_text(key, f.id)?
            .parse::<B256>()
            .map_err(|_| wrong())
    };
    let fh = hash("dex.factory_runtime_hash")?;
    let vh = hash("dex.quoter_runtime_hash")?;
    let mh = hash("dex.cl_fee_module_hash")?;
    let ih = hash("dex.pair_runtime_hash")?;
    let key = ICLFactory::getPoolCall {
        tokenA: pair.0,
        tokenB: pair.1,
        tickSpacing: super::load_cl::i24(ctx, spacing)?,
    };
    let calls = batch::read(
        ctx,
        [
            batch::view(at, factory, key.clone()),
            batch::view(at, factory, ITopaz::poolImplementationCall {}),
            batch::view(at, factory, ITopaz::swapFeeModuleCall {}),
            batch::view(at, module, ITopaz::factoryCall {}),
            batch::view(at, quoter, ITopaz::factoryCall {}),
            ("eth_getCode", json!([pool, at])),
        ],
    );
    let fee = rpc::call::<Bytes>(
        ctx,
        "eth_call",
        json!([{"from":sender(ctx),"to":pool,"data":Bytes::from(ICLPool::feeCall{}.abi_encode())},at]),
    );
    let (
        (),
        (),
        (),
        [
            market,
            impl_,
            fee_module,
            module_factory,
            quote_factory,
            code,
        ],
        rate,
    ) = tokio::try_join!(
        binding::plain(ctx, factory, fh, at),
        binding::plain(ctx, quoter, vh, at),
        binding::plain(ctx, module, mh, at),
        calls,
        fee
    )?;
    let implementation = batch::decode(&impl_, ITopaz::poolImplementationCall {})?;
    if implementation.is_zero()
        || super::verify_crypto_v1::proxy_target(&code) != Some(implementation)
        || batch::decode(&market, key)? != pool
        || batch::decode(&fee_module, ITopaz::swapFeeModuleCall {})? != module
        || batch::decode(&module_factory, ITopaz::factoryCall {})? != factory
        || batch::decode(&quote_factory, ITopaz::factoryCall {})? != factory
    {
        return Err(wrong());
    }
    binding::plain(ctx, implementation, ih, at).await?;
    let rate = batch::decode(&rate, ICLPool::feeCall {})?.to::<u32>();
    if rev != ctx.settings.generation() || rate >= 1_000_000 {
        return Err(wrong());
    }
    Ok(rate)
}

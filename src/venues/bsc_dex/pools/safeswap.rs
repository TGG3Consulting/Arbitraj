//! Неизменяемая пара SafeSwap рассчитывает K со ставкой из собственного байткода.
//! Принятый профиль связывает подтверждённую ставку с сетью, фабрикой и кодом пары.
use super::nami_batch as batch;
use crate::{
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::ctx::Ctx,
};
use alloy::sol_types::SolValue;
use alloy_primitives::{Address, B256, keccak256};
use rust_decimal::prelude::ToPrimitive;
use serde_json::Value;

pub(crate) fn fee_rule(chain: u64, factory: Address, init: B256, runtime: B256, pips: u32) -> B256 {
    keccak256((chain, factory, init, runtime, pips).abi_encode())
}

/// Both immutable pair code and the implementation resolving the factory registry must
/// match at the same canonical block. This V5 step transfers straight to the pair.
pub(in crate::venues::bsc_dex) async fn pair(
    ctx: &Ctx,
    f: &super::families::Family,
    pool: Address,
    (a, b): (Address, Address),
    at: &Value,
) -> GatewayResult<()> {
    use super::{abi_check::IPoolImmutables, abi_v2v3::IV2Factory, solidly_classic_factory};
    let generation = ctx.settings.generation();
    let bad = || {
        ctx.err(
            ErrorCode::NotApplicable,
            "SafeSwap: изменились проверенная фабрика, пара или комиссия",
        )
    };
    let factory = ctx.dex_addr("dex.factory", f.id)?;
    let pct = ctx.dex_dec("dex.v2_fee_pct", f.id)?;
    let pips = pct
        .checked_mul(rust_decimal::Decimal::from(10_000))
        .ok_or_else(bad)?;
    let fee = pips
        .to_u32()
        .filter(|v| *v < 1_000_000 && pips.fract().is_zero())
        .ok_or_else(bad)?;
    let pair_hash = ctx
        .dex_text("dex.pair_runtime_hash", f.id)?
        .parse::<B256>()
        .map_err(|_| bad())?;
    let accepted = ctx
        .dex_text("dex.v2_fee_rule_hash", f.id)?
        .parse::<B256>()
        .map_err(|_| bad())?;
    let hashes = ctx.dex_list("dex.pool_code_hashes", f.id)?;
    let [init] = hashes.as_slice() else {
        return Err(bad());
    };
    let init = init.parse::<B256>().map_err(|_| bad())?;
    let chain = ctx.chain_id().ok_or_else(bad)?;
    if a >= b
        || a.is_zero()
        || ctx.dex_list("dex.proxy_implementations", f.id)?.len() != 1
        || ctx.dex_text("dex.v2_fee_source", f.id)? != "fixed"
        || fee_rule(chain, factory, init, pair_hash, fee) != accepted
        || super::verify::v2_create2(factory, a, b, init) != pool
    {
        return Err(bad());
    }
    let listed = IV2Factory::getPairCall {
        tokenA: a,
        tokenB: b,
    };
    let owner = IPoolImmutables::factoryCall {};
    let (_, (), [registry, parent]) = tokio::try_join!(
        solidly_classic_factory::at(ctx, f, at),
        solidly_classic_factory::plain(ctx, pool, pair_hash, at),
        batch::read(
            ctx,
            [
                batch::view(at, factory, listed.clone()),
                batch::view(at, pool, owner.clone())
            ]
        )
    )?;
    if batch::decode(&registry, listed)? != pool
        || batch::decode(&parent, owner)? != factory
        || generation != ctx.settings.generation()
    {
        return Err(bad());
    }
    Ok(())
}

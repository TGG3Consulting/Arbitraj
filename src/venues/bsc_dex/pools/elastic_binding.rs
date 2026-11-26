//! Проверка фабрики, зарегистрированной пары и периферии Elastic V2 на одной канонической высоте.
use super::{families::Family, nami_batch as batch};
use crate::{
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::ctx::Ctx,
};
use alloy::sol;
use alloy_primitives::{Address, B256};
use serde_json::Value;

sol! {
    interface Elastic {
        function factory() external view returns(address);
        function token0() external view returns(address);
        function token1() external view returns(address);
        function swapFeeUnits() external view returns(uint24);
        function getPool(address a,address b,uint24 fee) external view returns(address);
    }
}
pub(in crate::venues::bsc_dex) async fn check(
    ctx: &Ctx,
    f: &Family,
    pool: Address,
    tag: &Value,
) -> GatewayResult<()> {
    let generation = ctx.settings.generation();
    let bad = || {
        ctx.err(
            ErrorCode::NotApplicable,
            "Elastic: код, фабрика или запись пула не совпали",
        )
    };
    let factory = super::solidly_classic_factory::at(ctx, f, tag).await?;
    let router = ctx.dex_addr("dex.router", f.id)?;
    let quoter = ctx.dex_addr("dex.quoter", f.id)?;
    let reader = ctx.dex_addr("dex.tick_reader", f.id)?;
    for (address, key) in [
        (router, "dex.router_runtime_hash"),
        (quoter, "dex.quoter_runtime_hash"),
        (reader, "dex.tick_reader_runtime_hash"),
    ] {
        let expected = ctx
            .dex_text(key, f.id)?
            .parse::<B256>()
            .map_err(|_| bad())?;
        super::solidly_classic_factory::plain(ctx, address, expected, tag).await?;
    }
    let [p, r, q, a, b, fee] = batch::read(
        ctx,
        [
            batch::view(tag, pool, Elastic::factoryCall {}),
            batch::view(tag, router, Elastic::factoryCall {}),
            batch::view(tag, quoter, Elastic::factoryCall {}),
            batch::view(tag, pool, Elastic::token0Call {}),
            batch::view(tag, pool, Elastic::token1Call {}),
            batch::view(tag, pool, Elastic::swapFeeUnitsCall {}),
        ],
    )
    .await?;
    for raw in [p, r, q] {
        if batch::decode(&raw, Elastic::factoryCall {})? != factory {
            return Err(bad());
        }
    }
    let a = batch::decode(&a, Elastic::token0Call {})?;
    let b = batch::decode(&b, Elastic::token1Call {})?;
    if a.is_zero() || a >= b {
        return Err(bad());
    }
    let fee = batch::decode(&fee, Elastic::swapFeeUnitsCall {})?;
    let lookup = Elastic::getPoolCall { a, b, fee };
    let [raw] = batch::read(ctx, [batch::view(tag, factory, lookup.clone())]).await?;
    if batch::decode(&raw, lookup)? != pool {
        return Err(bad());
    }
    if generation != ctx.settings.generation() {
        return Err(ctx.err(ErrorCode::StaleData, "Настройки Elastic изменились"));
    }
    Ok(())
}

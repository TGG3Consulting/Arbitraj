//! Maverick V1. Проверяет происхождение пары и PoolInformation на одном блоке.
use super::{
    families::Family,
    nami_batch as batch, solidly_classic_factory as binding,
    verify::{Candidate, Verdict},
};
use crate::{
    core::types::PoolRef,
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::ctx::Ctx,
};
use alloy_primitives::{Address, B256, U256};
use serde_json::Value;

alloy::sol! {
    interface IMaverickOne {
        event PoolCreated(address poolAddress,uint256 fee,uint256 tickSpacing,int32 activeTick,int256 lookback,uint64 protocolFeeRatio,address tokenA,address tokenB);
        function isFactoryPool(address pool) external view returns(bool);
        function factory() external view returns(address);
        function tokenA() external view returns(address);
        function tokenB() external view returns(address);
        function calculateSwap(address pool,uint128 amount,bool tokenAIn,bool exactOutput,uint256 sqrtPriceLimit) external returns(uint256);
    }
}

pub(crate) async fn market(
    ctx: &Ctx,
    f: &Family,
    pool: Address,
    pair: (Address, Address),
    at: &Value,
) -> GatewayResult<Address> {
    let generation = ctx.settings.generation();
    let invalid = || {
        ctx.err(
            ErrorCode::NotApplicable,
            "Maverick V1: реестр фабрики, код или токены рынка изменились",
        )
    };
    let factory = ctx.dex_addr("dex.factory", f.id)?;
    let viewer = ctx.dex_addr("dex.quoter", f.id)?;
    let hash = |key: &str| {
        ctx.dex_text(key, f.id)?
            .parse::<B256>()
            .map_err(|_| invalid())
    };
    let fh = hash("dex.factory_runtime_hash")?;
    let vh = hash("dex.quoter_runtime_hash")?;
    let who = IMaverickOne::isFactoryPoolCall { pool };
    let reads = batch::read(
        ctx,
        [
            batch::view(at, factory, who.clone()),
            batch::view(at, pool, IMaverickOne::factoryCall {}),
            batch::view(at, pool, IMaverickOne::tokenACall {}),
            batch::view(at, pool, IMaverickOne::tokenBCall {}),
        ],
    );
    let ((), (), [reg, origin, a, b]) = tokio::try_join!(
        binding::plain(ctx, factory, fh, at),
        binding::plain(ctx, viewer, vh, at),
        reads
    )?;
    if !batch::decode(&reg, who)?
        || batch::decode(&origin, IMaverickOne::factoryCall {})? != factory
        || batch::decode(&a, IMaverickOne::tokenACall {})? != pair.0
        || batch::decode(&b, IMaverickOne::tokenBCall {})? != pair.1
        || pair.0.is_zero()
        || pair.0 >= pair.1
        || generation != ctx.settings.generation()
    {
        return Err(invalid());
    }
    Ok(viewer)
}

pub(super) async fn verify(ctx: &Ctx, c: &Candidate) -> Verdict {
    let Ok(pool) = c.pool.parse::<Address>() else {
        return Verdict::Rejected("Maverick V1: требуется контракт пула".into());
    };
    let found = super::verify_woo::found(c, PoolRef::Contract(pool));
    let result = async {
        let at = super::nami::block(ctx, "latest").await?;
        market(ctx, c.family, pool, (found.token0, found.token1), &at).await
    }
    .await;
    match result {
        Ok(_) => Verdict::Taken(Box::new(found)),
        Err(e) if e.code == ErrorCode::NotApplicable => Verdict::Rejected(e.message),
        Err(e) => Verdict::Failed(e.message),
    }
}

pub(crate) fn quote(
    pool: Address,
    input_a: bool,
    amount: U256,
) -> Option<IMaverickOne::calculateSwapCall> {
    let amount = u128::try_from(amount).ok().filter(|a| *a > 0)?;
    Some(IMaverickOne::calculateSwapCall {
        pool,
        amount,
        tokenAIn: input_a,
        exactOutput: false,
        sqrtPriceLimit: U256::ZERO,
    })
}

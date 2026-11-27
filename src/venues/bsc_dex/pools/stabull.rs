//! Рынки Stabull. CurveFactoryV2 создаёт Curve через CREATE и регистрирует упорядоченную пару.
//! Цена использует текущие два assimilator и их оракулы. Проверки выполняются на едином каноническом блоке.
use super::{
    families::Family,
    nami_batch as batch, solidly_classic_factory,
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
    interface IStabull {
        event NewCurve(address indexed caller,bytes32 indexed id,address indexed curve);
        function getCurve(address base,address quote) external view returns(address);
        function derivatives(uint256 index) external view returns(address);
        function factory() external view returns(address);
        function getGlobalFrozenState() external view returns(bool);
        function frozen() external view returns(bool);
        function emergency() external view returns(bool);
        function assimilator(address asset) external view returns(address);
        function token() external view returns(address);
        function getRate() external view returns(uint256);
        function viewOriginSwap(address a,address b,uint256 amount) external view returns(uint256);
        function viewTargetSwap(address a,address b,uint256 amount) external view returns(uint256);
    }
}

fn invalid(ctx: &Ctx) -> crate::gateway::errors::GatewayError {
    ctx.err(
        ErrorCode::NotApplicable,
        "Stabull: реестр, торговое состояние или валюта расчёта изменились",
    )
}

pub(crate) async fn assets(ctx: &Ctx, pool: Address, at: &Value) -> GatewayResult<[Address; 2]> {
    let a = IStabull::derivativesCall { index: U256::ZERO };
    let b = IStabull::derivativesCall { index: U256::ONE };
    let [x, y] = batch::read(
        ctx,
        [
            batch::view(at, pool, a.clone()),
            batch::view(at, pool, b.clone()),
        ],
    )
    .await?;
    Ok([batch::decode(&x, a)?, batch::decode(&y, b)?])
}

pub(crate) async fn market(
    ctx: &Ctx,
    f: &Family,
    pool: Address,
    pair: (Address, Address),
    at: &Value,
) -> GatewayResult<()> {
    let generation = ctx.settings.generation();
    let factory = ctx.dex_addr("dex.factory", f.id)?;
    let hash = ctx
        .dex_text("dex.factory_runtime_hash", f.id)?
        .parse::<B256>()
        .map_err(|_| invalid(ctx))?;
    let ((), [a, b]) = tokio::try_join!(
        solidly_classic_factory::plain(ctx, factory, hash, at),
        assets(ctx, pool, at)
    )?;
    if a.is_zero() || b.is_zero() || a == b || (pair != (a, b) && pair != (b, a)) {
        return Err(invalid(ctx));
    }
    let key = IStabull::getCurveCall { base: a, quote: b };
    let first = IStabull::assimilatorCall { asset: a };
    let second = IStabull::assimilatorCall { asset: b };
    let [reg, global, frozen, emergency, x, y] = batch::read(
        ctx,
        [
            batch::view(at, factory, key.clone()),
            batch::view(at, factory, IStabull::getGlobalFrozenStateCall {}),
            batch::view(at, pool, IStabull::frozenCall {}),
            batch::view(at, pool, IStabull::emergencyCall {}),
            batch::view(at, pool, first.clone()),
            batch::view(at, pool, second.clone()),
        ],
    )
    .await?;
    if batch::decode(&reg, key)? != pool
        || batch::decode(&global, IStabull::getGlobalFrozenStateCall {})?
        || batch::decode(&frozen, IStabull::frozenCall {})?
        || batch::decode(&emergency, IStabull::emergencyCall {})?
    {
        return Err(invalid(ctx));
    }
    let x = batch::decode(&x, first)?;
    let y = batch::decode(&y, second)?;
    if x.is_zero() || y.is_zero() || x == y {
        return Err(invalid(ctx));
    }
    let [ta, tb, ra, rb] = batch::read(
        ctx,
        [
            batch::view(at, x, IStabull::tokenCall {}),
            batch::view(at, y, IStabull::tokenCall {}),
            batch::view(at, x, IStabull::getRateCall {}),
            batch::view(at, y, IStabull::getRateCall {}),
        ],
    )
    .await?;
    let rates = [
        batch::decode(&ra, IStabull::getRateCall {})?,
        batch::decode(&rb, IStabull::getRateCall {})?,
    ];
    if batch::decode(&ta, IStabull::tokenCall {})? != a
        || batch::decode(&tb, IStabull::tokenCall {})? != b
        || rates.iter().any(|r| r.is_zero() || r.bit(255))
        || generation != ctx.settings.generation()
    {
        return Err(invalid(ctx));
    }
    Ok(())
}

pub(super) async fn created(
    ctx: &Ctx,
    f: &'static Family,
    log: &alloy::rpc::types::Log,
    quote: Address,
) -> Result<Option<Candidate>, String> {
    use alloy::sol_types::{SolEvent, SolValue};
    let event = IStabull::NewCurve::decode_log_data(&log.inner.data).map_err(|e| e.to_string())?;
    let hash = log
        .block_hash
        .ok_or_else(|| "Stabull: нет хеша блока создания пары".to_string())?;
    let at = serde_json::json!({"blockHash":hash,"requireCanonical":true});
    let [a, b] = assets(ctx, event.curve, &at).await.map_err(|e| e.message)?;
    if alloy_primitives::keccak256((a, b).abi_encode()) != event.id {
        return Err("Stabull: ID фабрики не совпал с валютами Curve".into());
    }
    let coin = if a == quote {
        b
    } else if b == quote {
        a
    } else {
        return Ok(None);
    };
    Ok(Some(Candidate {
        family: f,
        coin,
        quote,
        pool: format!("{:#x}", event.curve),
    }))
}

pub(super) async fn verify(ctx: &Ctx, c: &Candidate) -> Verdict {
    let Ok(pool) = c.pool.parse::<Address>() else {
        return Verdict::Rejected("Stabull: нужен адрес Curve".into());
    };
    let found = super::verify_woo::found(c, PoolRef::Contract(pool));
    let result = async {
        let at = super::nami::block(ctx, "latest").await?;
        market(ctx, c.family, pool, (found.token0, found.token1), &at).await
    }
    .await;
    match result {
        Ok(()) => Verdict::Taken(Box::new(found)),
        Err(e) if e.code == ErrorCode::NotApplicable => Verdict::Rejected(e.message),
        Err(e) => Verdict::Failed(e.message),
    }
}

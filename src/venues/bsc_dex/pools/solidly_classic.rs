//! Состояние Solidly V1 Base. Фабрика, происхождение пары, обе стороны комиссии и обеспечение привязаны к одному блоку.
use super::{
    abi_solidly::{ISolidlyPool, IThenaV1Factory},
    families::Family,
    nami_batch as batch,
    registry::Entry,
    solidly_classic_factory, solidly_fee_read as fee,
};
use crate::{
    core::types::{PoolState, SolidlyGate},
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::{ctx::Ctx, node::abi::IERC20},
};
use alloy::{sol, sol_types::SolValue};
use alloy_primitives::{Address, B256, U256, keccak256};
use serde_json::{Value, json};

sol! {
    interface IPairGenerator {
        function pairGenerator() external view returns(address);
        function factory() external view returns(address);
        function pairCodeHash() external view returns(bytes32);
        function dibs() external view returns(address);
        function getReferralFee(address pool) external view returns(uint256);
    }
    interface IEqualizerFee {
        function feesOverrideStatic(address pool) external view returns(uint256);
        function feesOverrideDynamic(address pool) external view returns(address);
    }
}

async fn fee_origin(
    ctx: &Ctx,
    f: &Family,
    factory: Address,
    pool: Address,
    tag: &Value,
    source: &str,
) -> GatewayResult<()> {
    if source != "factory_address"
        || fee::optional(ctx, "dex.solidly_fee_method", f.id)?.as_deref()
            != Some("getRealFee(address)")
    {
        return Ok(());
    }
    let fixed = IEqualizerFee::feesOverrideStaticCall { pool };
    let manager = IEqualizerFee::feesOverrideDynamicCall { pool };
    let [val, addr] = batch::read(
        ctx,
        [
            batch::view(tag, factory, fixed.clone()),
            batch::view(tag, factory, manager.clone()),
        ],
    )
    .await?;
    if batch::decode(&val, fixed)?.is_zero() && !batch::decode(&addr, manager)?.is_zero() {
        return Err(ctx.err(
            ErrorCode::NotApplicable,
            "Solidly Classic: расчёт комиссии внешним менеджером этой пары не подтверждён",
        ));
    }
    Ok(())
}

pub fn address(factory: Address, a: Address, b: Address, stable: bool, init: B256) -> Address {
    let mut salt = [0u8; 41];
    salt[..20].copy_from_slice(a.as_slice());
    salt[20..40].copy_from_slice(b.as_slice());
    salt[40] = u8::from(stable);
    factory.create2(keccak256(salt), init)
}

async fn generator(
    ctx: &Ctx,
    f: &Family,
    factory: Address,
    pool: Address,
    accepted: &[B256],
    at: &Value,
) -> GatewayResult<Address> {
    let err = || {
        ctx.err(
            ErrorCode::NotApplicable,
            "Solidly PairGenerator: фабрика, шаблон пары или получатель комиссии изменились",
        )
    };
    let [target, owner, referral, rate] = batch::read(
        ctx,
        [
            batch::view(at, factory, IPairGenerator::pairGeneratorCall {}),
            batch::view(at, pool, IPairGenerator::factoryCall {}),
            batch::view(at, factory, IPairGenerator::dibsCall {}),
            batch::view(at, factory, IPairGenerator::getReferralFeeCall { pool }),
        ],
    )
    .await?;
    let target = batch::decode(&target, IPairGenerator::pairGeneratorCall {})?;
    if batch::decode(&owner, IPairGenerator::factoryCall {})? != factory
        || (batch::decode(&referral, IPairGenerator::dibsCall {})? == pool
            && !batch::decode(&rate, IPairGenerator::getReferralFeeCall { pool })?.is_zero())
    {
        return Err(err());
    }
    let hash = ctx
        .dex_text("dex.solidly_generator_runtime_hash", f.id)?
        .parse::<B256>()
        .map_err(|_| err())?;
    let ((), [origin, init]) = tokio::try_join!(
        solidly_classic_factory::plain(ctx, target, hash, at),
        batch::read(
            ctx,
            [
                batch::view(at, target, IPairGenerator::factoryCall {}),
                batch::view(at, target, IPairGenerator::pairCodeHashCall {})
            ]
        )
    )?;
    if batch::decode(&origin, IPairGenerator::factoryCall {})? != factory
        || !accepted.contains(&batch::decode(&init, IPairGenerator::pairCodeHashCall {})?)
    {
        return Err(err());
    }
    Ok(target)
}

pub(crate) async fn state(
    ctx: &Ctx,
    f: &Family,
    pool: Address,
    tag: &Value,
) -> GatewayResult<PoolState> {
    let generation = ctx.settings.generation();
    let factory = solidly_classic_factory::at(ctx, f, tag).await?;
    let accepted = super::verify_algebra_dir::hashes_for(ctx, f.id)
        .map_err(|e| ctx.err(ErrorCode::NotApplicable, e))?;
    let bad = || {
        ctx.err(
            ErrorCode::NotApplicable,
            "Solidly Classic: не подтверждены происхождение или обеспечение пары",
        )
    };
    let physical = if f.variant == super::family_variant::Variant::SolidlyGenerator {
        generator(ctx, f, factory, pool, &accepted, tag).await?
    } else {
        factory
    };
    let [metadata] =
        batch::read(ctx, [batch::view(tag, pool, ISolidlyPool::metadataCall {})]).await?;
    let m = batch::decode(&metadata, ISolidlyPool::metadataCall {})?;
    if m.t0.is_zero()
        || m.t0 >= m.t1
        || m.dec0.is_zero()
        || m.dec1.is_zero()
        || !accepted
            .iter()
            .any(|init| address(physical, m.t0, m.t1, m.st, *init) == pool)
    {
        return Err(bad());
    }
    let method = fee::source(ctx, f)?.ok_or_else(|| {
        ctx.not_set(
            "dex.solidly_fee_source",
            "Нужен подтверждённый метод комиссии Classic",
        )
    })?;
    fee_origin(ctx, f, factory, pool, tag, &method).await?;
    let rule = fee::rule(ctx, f, Some(&method))?;
    if f.variant == super::family_variant::Variant::SolidlyGenerator
        && (method != "factory_pool"
            || !matches!(
                rule,
                crate::core::types::FeeRule::Numerator { denominator: 10000 }
            )
            || fee::optional(ctx, "dex.solidly_pause_method", f.id)?.as_deref()
                != Some("isPaused()"))
    {
        return Err(bad());
    }
    let (target, wire) = fee::fee_call(ctx, f, Some(&method), (factory, pool, m.st))?;
    let pair = IThenaV1Factory::getPairCall {
        tokenA: m.t0,
        tokenB: m.t1,
        stable: m.st,
    };
    let bal = IERC20::balanceOfCall { owner: pool };
    let [listed, left, right, raw] = batch::read(
        ctx,
        [
            batch::view(tag, factory, pair.clone()),
            batch::view(tag, m.t0, bal.clone()),
            batch::view(tag, m.t1, bal.clone()),
            ("eth_call", json!([{"to":target,"data":wire},tag])),
        ],
    )
    .await?;
    if batch::decode(&listed, pair)? != pool
        || batch::decode(&left, bal.clone())? != m.r0
        || batch::decode(&right, bal)? != m.r1
    {
        return Err(bad());
    }
    let paused = if let Some(sig) = fee::optional(ctx, "dex.solidly_pause_method", f.id)? {
        let selector = alloy_primitives::Bytes::copy_from_slice(&fee::selector(&sig));
        let [word] = batch::read(
            ctx,
            [("eth_call", json!([{"to":factory,"data":selector},tag]))],
        )
        .await?;
        bool::abi_decode_validate(&word)
            .map_err(|e| ctx.err(ErrorCode::BadResponse, e.to_string()))?
    } else {
        false
    };
    let max = if let Some(sig) = fee::optional(ctx, "dex.solidly_max_gas_method", f.id)? {
        let selector = alloy_primitives::Bytes::copy_from_slice(&fee::selector(&sig));
        let [word] = batch::read(
            ctx,
            [("eth_call", json!([{"to":factory,"data":selector},tag]))],
        )
        .await?;
        fee::max_gas(
            U256::abi_decode_validate(&word)
                .map_err(|e| ctx.err(ErrorCode::BadResponse, e.to_string()))?,
        )
    } else {
        None
    };
    if generation != ctx.settings.generation() {
        return Err(ctx.err(
            ErrorCode::StaleData,
            "Solidly Classic: параметры изменились во время чтения",
        ));
    }
    Ok(PoolState::Solidly {
        reserve0: m.r0,
        reserve1: m.r1,
        stable: m.st,
        decimals0: m.dec0,
        decimals1: m.dec1,
        fee: fee::checked(ctx, fee::word(ctx, target, &raw)?, rule)?,
        fee1: None,
        gate: SolidlyGate {
            paused,
            max_gas_price: max,
        },
    })
}

pub(crate) async fn fresh(ctx: &Ctx, e: &Entry, tag: &Value) -> GatewayResult<SolidlyGate> {
    let current = state(ctx, e.family, e.pool.reference.address(), tag).await?;
    let bad = || {
        ctx.err(
            ErrorCode::NoRoute,
            "Solidly Classic: состояние или комиссия изменились после расчёта",
        )
    };
    if current != e.pool.state {
        return Err(bad());
    }
    let PoolState::Solidly { gate, .. } = current else {
        return Err(bad());
    };
    Ok(gate)
}

//! Канонический реестр минимальных клонов Solidly и штатная четырёхполевая форма Router.
use super::{
    abi_solidly::{ISolidlyFactory, ISolidlyPool},
    families::Family,
    nami_batch as batch,
    registry::Entry,
    solidly_classic_factory as binding,
};
use crate::{
    core::types::{FeeRule, PoolState, SolidlyGate},
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::{ctx::Ctx, node::abi::IERC20},
};
use alloy_primitives::{Address, B256, keccak256};
use serde_json::{Value, json};

alloy::sol! {
    interface CloneFactory {
        function implementation() external view returns(address);
        function isPaused() external view returns(bool);
        function factory() external view returns(address);
    }
    interface CloneRouter {
        function defaultFactory() external view returns(address);
        function factoryRegistry() external view returns(address);
        function isPoolFactoryApproved(address poolFactory) external view returns(bool);
        function poolFor(address tokenA,address tokenB,bool stable,address supplied) external view returns(address);
    }
}

pub(super) async fn state(
    ctx: &Ctx,
    f: &Family,
    pool: Address,
    factory: Address,
    at: &Value,
) -> GatewayResult<PoolState> {
    let generation = ctx.settings.generation();
    let bad = || {
        ctx.err(
            ErrorCode::NotApplicable,
            "Исходная фабрика, код клона, резервы или ставка Solidly изменились",
        )
    };
    let [meta, implementation, code, owner, listed, paused] = batch::read(
        ctx,
        [
            batch::view(at, pool, ISolidlyPool::metadataCall {}),
            batch::view(at, factory, CloneFactory::implementationCall {}),
            ("eth_getCode", json!([pool, at])),
            batch::view(at, pool, CloneFactory::factoryCall {}),
            batch::view(at, factory, ISolidlyFactory::isPoolCall { pool }),
            batch::view(at, factory, CloneFactory::isPausedCall {}),
        ],
    )
    .await?;
    let m = batch::decode(&meta, ISolidlyPool::metadataCall {})?;
    let imp = batch::decode(&implementation, CloneFactory::implementationCall {})?;
    let mut init = alloy_primitives::hex!("3d602d80600a3d3981f3").to_vec();
    init.extend_from_slice(&code);
    let digest = keccak256(init);
    if imp.is_zero()
        || super::verify_crypto_v1::proxy_target(&code) != Some(imp)
        || m.t0.is_zero()
        || m.t0 >= m.t1
        || m.dec0.is_zero()
        || m.dec1.is_zero()
        || super::solidly_classic::address(factory, m.t0, m.t1, m.st, digest) != pool
        || batch::decode(&owner, CloneFactory::factoryCall {})? != factory
        || !batch::decode(&listed, ISolidlyFactory::isPoolCall { pool })?
    {
        return Err(bad());
    }
    let resolve = |a, b| ISolidlyFactory::getPoolCall {
        tokenA: a,
        tokenB: b,
        stable: m.st,
    };
    let funds = IERC20::balanceOfCall { owner: pool };
    let fee = ISolidlyFactory::getFeeCall { pool, stable: m.st };
    let hash = ctx
        .dex_text("dex.pair_runtime_hash", f.id)?
        .parse::<B256>()
        .map_err(|_| bad())?;
    let ((), [forward, reverse, left, right, rate]) = tokio::try_join!(
        binding::plain(ctx, imp, hash, at),
        batch::read(
            ctx,
            [
                batch::view(at, factory, resolve(m.t0, m.t1)),
                batch::view(at, factory, resolve(m.t1, m.t0)),
                batch::view(at, m.t0, funds.clone()),
                batch::view(at, m.t1, funds.clone()),
                batch::view(at, factory, fee.clone())
            ]
        )
    )?;
    if batch::decode(&forward, resolve(m.t0, m.t1))? != pool
        || batch::decode(&reverse, resolve(m.t1, m.t0))? != pool
        || batch::decode(&left, funds.clone())? != m.r0
        || batch::decode(&right, funds)? != m.r1
        || generation != ctx.settings.generation()
    {
        return Err(bad());
    }
    Ok(PoolState::Solidly {
        reserve0: m.r0,
        reserve1: m.r1,
        stable: m.st,
        decimals0: m.dec0,
        decimals1: m.dec1,
        fee: super::solidly_fee_read::checked(
            ctx,
            batch::decode(&rate, fee)?,
            FeeRule::Numerator {
                denominator: crate::core::types::pool_solidly::GETFEE_DENOMINATOR,
            },
        )?,
        fee1: None,
        gate: SolidlyGate {
            paused: batch::decode(&paused, CloneFactory::isPausedCall {})?,
            max_gas_price: None,
        },
    })
}

pub(super) async fn router(
    ctx: &Ctx,
    e: &Entry,
    input: Address,
    output: Address,
    at: &Value,
) -> GatewayResult<Address> {
    let id = e.family.id;
    let generation = ctx.settings.generation();
    let fail = || {
        ctx.err(
            ErrorCode::NoRoute,
            "Роутер Solidly или маршрут оплаты изменился",
        )
    };
    let factory = ctx.dex_addr("dex.factory", id)?;
    let target = ctx.dex_addr("dex.router", id)?;
    let hash = ctx
        .dex_text("dex.router_runtime_hash", id)?
        .parse::<B256>()
        .map_err(|_| fail())?;
    let PoolState::Solidly { stable, .. } = &e.pool.state else {
        return Err(fail());
    };
    if (input.min(output), input.max(output)) != (e.pool.token0, e.pool.token1) {
        return Err(fail());
    }
    let registered = CloneRouter::poolForCall {
        tokenA: input,
        tokenB: output,
        stable: *stable,
        supplied: factory,
    };
    let (gate, (), (), [owner, pool, registry]) = tokio::try_join!(
        Box::pin(super::solidly_classic::fresh(ctx, e, at)),
        binding::plain(ctx, target, hash, at),
        super::exact_wallet_tokens::check_input(ctx, id, input, at),
        batch::read(
            ctx,
            [
                batch::view(at, target, CloneRouter::defaultFactoryCall {}),
                batch::view(at, target, registered.clone()),
                batch::view(at, target, CloneRouter::factoryRegistryCall {})
            ]
        )
    )?;
    let registry = batch::decode(&registry, CloneRouter::factoryRegistryCall {})?;
    let approved = CloneRouter::isPoolFactoryApprovedCall {
        poolFactory: factory,
    };
    let [enabled] = batch::read(ctx, [batch::view(at, registry, approved.clone())]).await?;
    if gate.paused
        || batch::decode(&owner, CloneRouter::defaultFactoryCall {})? != factory
        || batch::decode(&pool, registered)? != e.pool.reference.address()
        || !batch::decode(&enabled, approved)?
        || generation != ctx.settings.generation()
    {
        return Err(fail());
    }
    Ok(target)
}

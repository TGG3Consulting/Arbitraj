//! Solidly Ethereum V2. The parent and child proxies delegate through two distinct role getters.
use super::{
    abi_solidly::IThenaV1Factory, families::Family, nami_batch as batch, solidly_classic,
    solidly_classic_factory::runtime, solidly_fee_read, verify_algebra_dir,
};
use crate::{
    core::types::{FeeRule, PoolState, SolidlyGate},
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::{ctx::Ctx, node::abi::IERC20},
};
use alloy::sol;
use alloy_primitives::{Address, B256};
use serde_json::Value;

sol! {
    interface ISolidlyProxy {
        function factory() external view returns (address);
        function pairFor(address a,address b,bool stable) external view returns(address);
        function logicAddress() external view returns (address);
        function childSubImplementationAddress() external view returns (address);
        function factoryAddress() external view returns (address);
        function fees() external view returns (address);
        function isPaused() external view returns (bool);
        function metadata() external view returns (uint256 dec0, uint256 dec1, uint256 r0, uint256 r1,
            bool stable, address t0, address t1, uint256 feePpm);
    }
}

pub(super) async fn state(
    ctx: &Ctx,
    f: &Family,
    pool: Address,
    at: &Value,
) -> GatewayResult<PoolState> {
    let generation = ctx.settings.generation();
    let bad = || {
        ctx.err(
            ErrorCode::NotApplicable,
            "Solidly V2: изменилась привязка фабрики, пары или логики обмена",
        )
    };
    let config = || {
        ctx.err(
            ErrorCode::NotConfigured,
            "Solidly V2: нужны адреса и хеши обеих реализаций в порядке фабрика, пара",
        )
    };
    let factory = ctx.dex_addr("dex.factory", f.id)?;
    let roots = ctx.dex_list("dex.proxy_implementations", f.id)?;
    let hashes = ctx.dex_list("dex.proxy_runtime_hashes", f.id)?;
    let (Some([fac_logic, pair_logic]), Some([fac_hash, pair_hash])) =
        (roots.get(..2), hashes.get(..2))
    else {
        return Err(config());
    };
    let fac_logic: Address = fac_logic.parse().map_err(|_| config())?;
    let pair_logic: Address = pair_logic.parse().map_err(|_| config())?;
    let fac_hash: B256 = fac_hash.parse().map_err(|_| config())?;
    let pair_hash: B256 = pair_hash.parse().map_err(|_| config())?;
    let shell =
        |key| -> GatewayResult<B256> { ctx.dex_text(key, f.id)?.parse().map_err(|_| config()) };
    if fac_logic == pair_logic
        || [fac_logic, pair_logic]
            .iter()
            .any(|a| *a == factory || *a == pool)
    {
        return Err(config());
    }
    let allowed = verify_algebra_dir::hashes_for(ctx, f.id)
        .map_err(|e| ctx.err(ErrorCode::NotConfigured, e))?;
    let ((), (), (), (), [a, b, c, m, paused, fees]) = tokio::try_join!(
        runtime(ctx, factory, shell("dex.factory_runtime_hash")?, at),
        runtime(ctx, pool, shell("dex.pair_runtime_hash")?, at),
        runtime(ctx, fac_logic, fac_hash, at),
        runtime(ctx, pair_logic, pair_hash, at),
        batch::read(
            ctx,
            [
                batch::view(at, factory, ISolidlyProxy::logicAddressCall {}),
                batch::view(
                    at,
                    factory,
                    ISolidlyProxy::childSubImplementationAddressCall {}
                ),
                batch::view(at, pool, ISolidlyProxy::factoryAddressCall {}),
                batch::view(at, pool, ISolidlyProxy::metadataCall {}),
                batch::view(at, factory, ISolidlyProxy::isPausedCall {}),
                batch::view(at, pool, ISolidlyProxy::feesCall {}),
            ]
        )
    )?;
    let m = batch::decode(&m, ISolidlyProxy::metadataCall {})?;
    if batch::decode(&a, ISolidlyProxy::logicAddressCall {})? != fac_logic
        || batch::decode(&b, ISolidlyProxy::childSubImplementationAddressCall {})? != pair_logic
        || batch::decode(&c, ISolidlyProxy::factoryAddressCall {})? != factory
        || batch::decode(&fees, ISolidlyProxy::feesCall {})? == pool
        || m.t0.is_zero()
        || m.t0 >= m.t1
        || m.dec0.is_zero()
        || m.dec1.is_zero()
        || !allowed
            .iter()
            .any(|h| solidly_classic::address(factory, m.t0, m.t1, m.stable, *h) == pool)
    {
        return Err(bad());
    }
    let registered = |x, y| IThenaV1Factory::getPairCall {
        tokenA: x,
        tokenB: y,
        stable: m.stable,
    };
    let listed = IThenaV1Factory::isPairCall { pair: pool };
    let balance = IERC20::balanceOfCall { owner: pool };
    let [ab, ba, is_pair, left, right] = batch::read(
        ctx,
        [
            batch::view(at, factory, registered(m.t0, m.t1)),
            batch::view(at, factory, registered(m.t1, m.t0)),
            batch::view(at, factory, listed.clone()),
            batch::view(at, m.t0, balance.clone()),
            batch::view(at, m.t1, balance.clone()),
        ],
    )
    .await?;
    if batch::decode(&ab, registered(m.t0, m.t1))? != pool
        || batch::decode(&ba, registered(m.t1, m.t0))? != pool
        || !batch::decode(&is_pair, listed)?
        || batch::decode(&left, balance.clone())? != m.r0
        || batch::decode(&right, balance)? != m.r1
        || generation != ctx.settings.generation()
    {
        return Err(bad());
    }
    Ok(PoolState::Solidly {
        reserve0: m.r0,
        reserve1: m.r1,
        stable: m.stable,
        decimals0: m.dec0,
        decimals1: m.dec1,
        fee: solidly_fee_read::checked(
            ctx,
            m.feePpm,
            FeeRule::Numerator {
                denominator: 1_000_000,
            },
        )?,
        fee1: None,
        gate: SolidlyGate {
            paused: batch::decode(&paused, ISolidlyProxy::isPausedCall {})?,
            max_gas_price: None,
        },
    })
}

/// Router01 has its own proxy. Its implementation and pool derivation are independent from V5 admission.
pub(in crate::venues::bsc_dex) async fn router(
    ctx: &Ctx,
    e: &super::registry::Entry,
    input: Address,
    output: Address,
    at: &Value,
) -> GatewayResult<Address> {
    let id = e.family.id;
    let bad = || {
        ctx.err(
            ErrorCode::NotApplicable,
            "Solidly Router01: изменились фабрика, расчёт адреса пары или исполняемый код",
        )
    };
    if e.family.variant == super::family_variant::Variant::SolidlyPairOverride {
        if !((input, output) == (e.pool.token0, e.pool.token1)
            || (output, input) == (e.pool.token0, e.pool.token1))
        {
            return Err(bad());
        }
        let generation = ctx.settings.generation();
        let target = ctx.dex_addr("dex.router", id)?;
        let hash = ctx
            .dex_text("dex.router_runtime_hash", id)?
            .parse::<B256>()
            .map_err(|_| bad())?;
        let (gate, (), (), [owner]) = tokio::try_join!(
            Box::pin(super::solidly_classic::fresh(ctx, e, at)),
            super::solidly_classic_factory::plain(ctx, target, hash, at),
            super::exact_wallet_tokens::check_input(ctx, id, input, at),
            super::nami_batch::read(
                ctx,
                [super::nami_batch::view(
                    at,
                    target,
                    super::abi_check::IPoolImmutables::factoryCall {}
                )]
            )
        )?;
        if gate.paused
            || super::nami_batch::decode(&owner, super::abi_check::IPoolImmutables::factoryCall {})?
                != ctx.dex_addr("dex.factory", id)?
            || generation != ctx.settings.generation()
        {
            return Err(bad());
        }
        return Ok(target);
    }
    if e.family.variant != super::family_variant::Variant::SolidlyProxy
        || !((input, output) == (e.pool.token0, e.pool.token1)
            || (output, input) == (e.pool.token0, e.pool.token1))
    {
        return Err(bad());
    }
    let generation = ctx.settings.generation();
    let target = ctx.dex_addr("dex.router", id)?;
    let hash: B256 = ctx
        .dex_text("dex.router_runtime_hash", id)?
        .parse()
        .map_err(|_| bad())?;
    let logic: Address = ctx
        .dex_list("dex.proxy_implementations", id)?
        .get(2)
        .ok_or_else(bad)?
        .parse()
        .map_err(|_| bad())?;
    let logic_hash: B256 = ctx
        .dex_list("dex.proxy_runtime_hashes", id)?
        .get(2)
        .ok_or_else(bad)?
        .parse()
        .map_err(|_| bad())?;
    let PoolState::Solidly { stable, .. } = &e.pool.state else {
        return Err(bad());
    };
    let pair = ISolidlyProxy::pairForCall {
        a: input,
        b: output,
        stable: *stable,
    };
    let (gate, (), (), (), [fac, implementation, pool]) = tokio::try_join!(
        solidly_classic::fresh(ctx, e, at),
        runtime(ctx, target, hash, at),
        runtime(ctx, logic, logic_hash, at),
        super::exact_wallet_tokens::check(ctx, id, input, output, at),
        batch::read(
            ctx,
            [
                batch::view(at, target, ISolidlyProxy::factoryCall {}),
                batch::view(at, target, ISolidlyProxy::logicAddressCall {}),
                batch::view(at, target, pair.clone())
            ]
        )
    )?;
    if gate.paused
        || batch::decode(&fac, ISolidlyProxy::factoryCall {})? != ctx.dex_addr("dex.factory", id)?
        || batch::decode(&implementation, ISolidlyProxy::logicAddressCall {})? != logic
        || batch::decode(&pool, pair)? != e.pool.reference.address()
        || ctx.settings.generation() != generation
    {
        return Err(bad());
    }
    Ok(target)
}

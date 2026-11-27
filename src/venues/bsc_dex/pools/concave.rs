//! Рынок Gemswap типа 1. У клона нет factory(): тип, CREATE2, обе регистрации и шаблон EIP-1167 привязываются к одному блоку.
use super::{
    abi_check::IPoolImmutables,
    abi_v2v3::{IV2Factory, IV2Pair},
    families::Family,
    nami_batch as batch,
    registry::Entry,
    verify::Candidate,
};
use crate::{
    core::types::{PoolRef, PoolState},
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::{ctx::Ctx, node::abi::IERC20},
};
use alloy_primitives::{Address, B256, U256, keccak256};
use serde_json::{Value, json};

alloy::sol! {
    interface IGemswap {
        function implementation() external view returns(address);
        function typeToImplementation(uint256 kind) external view returns(address);
        function getPair(address a,address b,uint256 kind) external view returns(address);
        function swapFee() external view returns(uint256);
    }
}

fn clone(
    factory: Address,
    implementation: Address,
    tokens: (Address, Address),
) -> (Address, [u8; 45]) {
    let mut init = [0u8; 55];
    init[..20].copy_from_slice(&alloy_primitives::hex!(
        "3d602d80600a3d3981f3363d3d373d3d3d363d73"
    ));
    init[20..40].copy_from_slice(implementation.as_slice());
    init[40..].copy_from_slice(&alloy_primitives::hex!("5af43d82803e903d91602b57fd5bf3"));
    let mut runtime = [0u8; 45];
    runtime.copy_from_slice(&init[10..]);
    let mut key = [0u8; 72];
    key[..20].copy_from_slice(tokens.0.as_slice());
    key[20..40].copy_from_slice(tokens.1.as_slice());
    key[71] = 1;
    (factory.create2(keccak256(key), keccak256(init)), runtime)
}

fn configured_implementation(ctx: &Ctx, id: &str) -> GatewayResult<(Address, B256)> {
    let addrs = ctx.dex_list("dex.proxy_implementations", id)?;
    let hashes = ctx.dex_list("dex.proxy_runtime_hashes", id)?;
    if let ([addr], [hash]) = (addrs.as_slice(), hashes.as_slice())
        && let (Ok(a), Ok(h)) = (addr.parse::<Address>(), hash.parse::<B256>())
        && !a.is_zero()
        && !h.is_zero()
    {
        return Ok((a, h));
    }
    Err(ctx.err(
        ErrorCode::NotConfigured,
        "Gemswap: нужны реализация типа 1 и её runtime hash",
    ))
}

pub(crate) struct Market {
    pub reserves: (U256, U256),
    pub balances: (U256, U256),
    pub fee: u32,
}

pub(crate) async fn market(
    ctx: &Ctx,
    f: &Family,
    pool: Address,
    tokens: (Address, Address),
    at: &Value,
) -> GatewayResult<Market> {
    let generation = ctx.settings.generation();
    let bad = || {
        ctx.err(
            ErrorCode::NotApplicable,
            "Gemswap: регистрация, реализация, ставка или состав пары изменились",
        )
    };
    let id = f.id;
    let factory = ctx.dex_addr("dex.factory", id)?;
    let (implementation, implementation_hash) = configured_implementation(ctx, id)?;
    let hash = ctx
        .dex_text("dex.factory_runtime_hash", id)?
        .parse::<B256>()
        .map_err(|_| bad())?;
    let (predicted, template) = clone(factory, implementation, tokens);
    if tokens.0.is_zero() || tokens.0 >= tokens.1 || predicted != pool {
        return Err(bad());
    }
    let registered = |a, b| {
        batch::view(
            at,
            factory,
            IGemswap::getPairCall {
                a,
                b,
                kind: U256::ONE,
            },
        )
    };
    let balance = IERC20::balanceOfCall { owner: pool };
    let fields = batch::read(
        ctx,
        [
            ("eth_getCode", json!([pool, at])),
            batch::view(
                at,
                factory,
                IGemswap::typeToImplementationCall { kind: U256::ONE },
            ),
            registered(tokens.0, tokens.1),
            registered(tokens.1, tokens.0),
            batch::view(
                at,
                factory,
                IV2Factory::getPairCall {
                    tokenA: tokens.0,
                    tokenB: tokens.1,
                },
            ),
            batch::view(at, pool, IPoolImmutables::token0Call {}),
            batch::view(at, pool, IPoolImmutables::token1Call {}),
            batch::view(at, pool, IGemswap::swapFeeCall {}),
            batch::view(at, pool, IV2Pair::getReservesCall {}),
            batch::view(at, tokens.0, balance.clone()),
            batch::view(at, tokens.1, balance.clone()),
        ],
    );
    let (
        (),
        (),
        [
            code,
            role,
            forward,
            reverse,
            default,
            a,
            b,
            fee,
            reserves,
            b0,
            b1,
        ],
    ) = tokio::try_join!(
        super::solidly_classic_factory::plain(ctx, factory, hash, at),
        super::solidly_classic_factory::plain(ctx, implementation, implementation_hash, at),
        fields
    )?;
    let registry = IGemswap::getPairCall {
        a: tokens.0,
        b: tokens.1,
        kind: U256::ONE,
    };
    if code.as_ref() != template.as_slice()
        || batch::decode(
            &role,
            IGemswap::typeToImplementationCall { kind: U256::ONE },
        )? != implementation
        || batch::decode(&forward, registry.clone())? != pool
        || batch::decode(&reverse, registry)? != pool
        || batch::decode(
            &default,
            IV2Factory::getPairCall {
                tokenA: tokens.0,
                tokenB: tokens.1,
            },
        )? != pool
        || (
            batch::decode(&a, IPoolImmutables::token0Call {})?,
            batch::decode(&b, IPoolImmutables::token1Call {})?,
        ) != tokens
        || batch::decode(&fee, IGemswap::swapFeeCall {})? != U256::from(25)
        || generation != ctx.settings.generation()
    {
        return Err(bad());
    }
    let rs = batch::decode(&reserves, IV2Pair::getReservesCall {})?;
    Ok(Market {
        reserves: (U256::from(rs.reserve0), U256::from(rs.reserve1)),
        balances: (
            batch::decode(&b0, balance.clone())?,
            batch::decode(&b1, balance)?,
        ),
        fee: 2500,
    })
}

pub(crate) async fn checked(
    ctx: &Ctx,
    e: &Entry,
    with_router: bool,
    at: &Value,
) -> GatewayResult<()> {
    let generation = ctx.settings.generation();
    let m = market(
        ctx,
        e.family,
        e.pool.reference.address(),
        (e.pool.token0, e.pool.token1),
        at,
    )
    .await?;
    let bad = || {
        ctx.err(
            ErrorCode::QuoteMismatch,
            "Gemswap: изменились резервы, комиссия или активный Router",
        )
    };
    if !matches!(e.pool.state,PoolState::V2{reserve0,reserve1} if (reserve0,reserve1)==m.reserves)
        || e.pool.fee_pips != Some(m.fee)
        || m.reserves != m.balances
    {
        return Err(bad());
    }
    if with_router {
        let id = e.family.id;
        let router = ctx.dex_addr("dex.router", id)?;
        let hash = ctx
            .dex_text("dex.router_runtime_hash", id)?
            .parse::<B256>()
            .map_err(|_| bad())?;
        let (implementation, _) = configured_implementation(ctx, id)?;
        let [fact, imp] = batch::read(
            ctx,
            [
                batch::view(at, router, IPoolImmutables::factoryCall {}),
                batch::view(at, router, IGemswap::implementationCall {}),
            ],
        )
        .await?;
        super::solidly_classic_factory::plain(ctx, router, hash, at).await?;
        if batch::decode(&fact, IPoolImmutables::factoryCall {})?
            != ctx.dex_addr("dex.factory", id)?
            || batch::decode(&imp, IGemswap::implementationCall {})? != implementation
        {
            return Err(bad());
        }
    }
    if generation != ctx.settings.generation() {
        return Err(bad());
    }
    Ok(())
}

pub(crate) async fn verify(
    ctx: &Ctx,
    c: &Candidate,
    pool: Address,
) -> GatewayResult<super::discover::Found> {
    let at = super::nami::block(ctx, "latest").await?;
    let market = market(
        ctx,
        c.family,
        pool,
        super::discover::sorted(c.coin, c.quote),
        &at,
    )
    .await?;
    let mut found = super::verify_woo::found(c, PoolRef::Contract(pool));
    found.fee_pips = Some(market.fee);
    Ok(found)
}

#[cfg(test)]
#[path = "concave_tests.rs"]
mod tests;

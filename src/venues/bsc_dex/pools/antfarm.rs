//! Read the authenticated Antfarm pair and fee oracle at one canonical Ethereum block.
use super::super::{ctx::Ctx, math::antfarm::Market};
use super::antfarm_profile::Profile;
use super::{families::Family, nami_batch as batch};
use crate::gateway::errors::{ErrorCode, GatewayResult};
use alloy::sol;
use alloy_primitives::{Address, B256, U256};
use serde_json::Value;

sol! {
    interface IFarm {
        function getPair(address a,address b,uint16 fee) external view returns(address);
        function getFeesForPair(address token0,address token1) external view returns(uint16[8]);
        function price1CumulativeLast() external view returns(uint256);
        function blockTimestampLast() external view returns(uint32);
        function firstUpdateCall() external view returns(bool);
        function token0() external view returns(address);
        function token1() external view returns(address);
        function factory() external view returns(address);
        function antfarmToken() external view returns(address);
        function fee() external view returns(uint16);
        function getReserves() external view returns(uint112 x,uint112 y,uint32 last);
        function totalSupply() external view returns(uint256);
        function antfarmTokenReserve() external view returns(uint256);
        function antfarmOracle() external view returns(address);
        function pair() external view returns(address);
        function price1Average() external view returns(uint224);
        function balanceOf(address account) external view returns(uint256);
    }
}

pub(super) async fn market(
    ctx: &Ctx,
    f: &Family,
    pool: Address,
    edge: (Address, Address),
    at: &Value,
) -> GatewayResult<Market> {
    let profile = Profile::read(ctx, f.id)?;
    bound_market(ctx, profile, pool, edge, at).await
}
pub(super) async fn bound_market(
    ctx: &Ctx,
    p: Profile,
    pool: Address,
    edge: (Address, Address),
    at: &Value,
) -> GatewayResult<Market> {
    let bad = || {
        ctx.err(ErrorCode::NotApplicable,"Antfarm: сеть, пара, код контракта или источник комиссии расходятся с реестром Ethereum")
    };
    let generation = ctx.settings.generation();
    if ctx.chain_id() != Some(1)
        || at["requireCanonical"] != true
        || at["blockHash"]
            .as_str()
            .and_then(|s| s.parse::<B256>().ok())
            .is_none()
        || edge.0 == edge.1
        || pool.is_zero()
    {
        return Err(bad());
    }
    let roles = batch::read(
        ctx,
        [
            batch::view(at, pool, IFarm::token0Call {}),
            batch::view(at, pool, IFarm::token1Call {}),
            batch::view(at, pool, IFarm::feeCall {}),
            batch::view(at, pool, IFarm::factoryCall {}),
            batch::view(at, pool, IFarm::getReservesCall {}),
            batch::view(at, pool, IFarm::totalSupplyCall {}),
            batch::view(at, pool, IFarm::antfarmTokenReserveCall {}),
            batch::view(at, pool, IFarm::antfarmOracleCall {}),
        ],
    );
    let bind = |a, h| super::solidly_classic_factory::bind(ctx, a, h, None, at);
    let (_, _, _, [a, b, fee, owner, res, supply, escrow, oracle]) = tokio::try_join!(
        bind(p.factory, p.factory_hash),
        bind(p.router, p.router_hash),
        bind(
            pool,
            p.runtime[usize::from(edge.0 == p.atf || edge.1 == p.atf)]
        ),
        roles
    )?;
    let a = batch::decode(&a, IFarm::token0Call {})?;
    let b = batch::decode(&b, IFarm::token1Call {})?;
    let tier = batch::decode(&fee, IFarm::feeCall {})?;
    let embedded = a == p.atf;
    if (a, b) != edge && (b, a) != edge
        || b == p.atf
        || !embedded && a >= b
        || batch::decode(&owner, IFarm::factoryCall {})? != p.factory
        || p.derived([a, b], tier) != pool
    {
        return Err(bad());
    }
    let reg = |x, y| IFarm::getPairCall {
        a: x,
        b: y,
        fee: tier,
    };
    let bal = IFarm::balanceOfCall { account: pool };
    let [
        ab,
        ba,
        router_atf,
        router_factory,
        factory_atf,
        b0,
        b1,
        bfee,
    ] = batch::read(
        ctx,
        [
            batch::view(at, p.factory, reg(a, b)),
            batch::view(at, p.factory, reg(b, a)),
            batch::view(at, p.router, IFarm::antfarmTokenCall {}),
            batch::view(at, p.router, IFarm::factoryCall {}),
            batch::view(at, p.factory, IFarm::antfarmTokenCall {}),
            batch::view(at, a, bal.clone()),
            batch::view(at, b, bal.clone()),
            batch::view(at, p.atf, bal.clone()),
        ],
    )
    .await?;
    if batch::decode(&ab, reg(a, b))? != pool
        || batch::decode(&ba, reg(b, a))? != pool
        || batch::decode(&router_factory, IFarm::factoryCall {})? != p.factory
        || batch::decode(&factory_atf, IFarm::antfarmTokenCall {})? != p.atf
        || batch::decode(&router_atf, IFarm::antfarmTokenCall {})? != p.atf
    {
        return Err(bad());
    }
    let oracle = batch::decode(&oracle, IFarm::antfarmOracleCall {})?;
    let rate = if embedded || oracle.is_zero() {
        None
    } else {
        let [src, coin, rate] = batch::read(
            ctx,
            [
                batch::view(at, oracle, IFarm::pairCall {}),
                batch::view(at, oracle, IFarm::token1Call {}),
                batch::view(at, oracle, IFarm::price1AverageCall {}),
            ],
        )
        .await?;
        let src = batch::decode(&src, IFarm::pairCall {})?;
        let coin = batch::decode(&coin, IFarm::token1Call {})?;
        let index = [a, b].iter().position(|t| *t == coin).ok_or_else(bad)?;
        if src != p.derived([p.atf, coin], 10) {
            return Err(bad());
        }
        let [back, registered] = batch::read(
            ctx,
            [
                batch::view(at, src, IFarm::antfarmOracleCall {}),
                batch::view(
                    at,
                    p.factory,
                    IFarm::getPairCall {
                        a: p.atf,
                        b: coin,
                        fee: 10,
                    },
                ),
            ],
        )
        .await?;
        if batch::decode(&back, IFarm::antfarmOracleCall {})? != oracle
            || batch::decode(
                &registered,
                IFarm::getPairCall {
                    a: p.atf,
                    b: coin,
                    fee: 10,
                },
            )? != src
        {
            return Err(bad());
        }
        Some((
            index,
            U256::from(batch::decode(&rate, IFarm::price1AverageCall {})?),
        ))
    };
    if generation != ctx.settings.generation() {
        return Err(ctx.err(
            ErrorCode::StaleData,
            "Antfarm: профиль изменился во время чтения",
        ));
    }
    let r = batch::decode(&res, IFarm::getReservesCall {})?;
    Ok(Market {
        pool,
        tokens: [a, b],
        atf: p.atf,
        tier,
        reserves: [U256::from(r.x), U256::from(r.y)],
        balances: [
            batch::decode(&b0, bal.clone())?,
            batch::decode(&b1, bal.clone())?,
            batch::decode(&bfee, bal)?,
        ],
        lp_supply: batch::decode(&supply, IFarm::totalSupplyCall {})?,
        dividends: batch::decode(&escrow, IFarm::antfarmTokenReserveCall {})?,
        oracle: rate,
        oracle_address: (!oracle.is_zero()).then_some(oracle),
    })
}

pub(super) async fn verify(ctx: &Ctx, c: &super::verify::Candidate) -> super::verify::Verdict {
    use super::verify::Verdict;
    use crate::core::types::PoolRef;
    let Ok(pool) = c.pool.parse::<Address>() else {
        return Verdict::Rejected("Antfarm: адрес пары неверен".into());
    };
    let read = async {
        let at = super::nami::block(ctx, "latest").await?;
        market(ctx, c.family, pool, (c.coin, c.quote), &at).await
    };
    match read.await {
        Ok(s) => {
            let mut f = super::verify_woo::found(c, PoolRef::Contract(pool));
            [f.token0, f.token1] = s.tokens;
            Verdict::Taken(Box::new(f))
        }
        Err(e) if e.code == ErrorCode::NotApplicable => Verdict::Rejected(e.message),
        Err(e) => Verdict::Failed(e.message),
    }
}

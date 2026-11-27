//! Read the authenticated Antfarm pair and fee oracle at one canonical Ethereum block.
use super::super::{ctx::Ctx, math::antfarm::Market};
use super::{families::Family, nami_batch as batch};
use crate::gateway::errors::{ErrorCode, GatewayResult};
use alloy::sol;
use alloy_primitives::{Address, B256, U256, address, b256, keccak256};
use serde_json::Value;

pub(super) const FACTORY: Address = address!("e48aee124f9933661d4dd3eb265fa9e153e32cbe");
pub(super) const ROUTER: Address = address!("6d9f0eb21d77c6d24be49a579508471e937d5418");
pub(super) const ATF: Address = address!("518b63da813d46556fea041a88b52e3caa8c16a8");
const FACTORY_CODE: B256 =
    b256!("f2d8ede580acbe903f4236f3ea575e17dcdbfb7369a9fcf433b863d010e197fc");
const ROUTER_CODE: B256 = b256!("ab093829729d83c3859f76c3343b65622c0cd740c2b40a661bc2e0b5f6a45cb4");
const PAIR_CODE: [B256; 2] = [
    b256!("c7e0d34bd74d343e4a4e51a38239e7bd12d95de9570a730445913fcf8a08086d"),
    b256!("359924a3c31734039b5b61c2442549d28b1697852bff9c3dcfdcef0c27d94f03"),
];
const INIT: [B256; 2] = [
    b256!("2f47d72b208014a5ba4f32371ac96dd421a39152dcaf104e8232b6c9f1a92280"),
    b256!("b174de46ec9038ead3d74ed04c79d4885d8e642175833c4da037d5e052492e5b"),
];

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

pub(super) fn derived(tokens: [Address; 2], tier: u16) -> Address {
    let mut salt = [0u8; 62];
    salt[..20].copy_from_slice(tokens[0].as_slice());
    salt[20..40].copy_from_slice(tokens[1].as_slice());
    salt[40..42].copy_from_slice(&tier.to_be_bytes());
    salt[42..].copy_from_slice(ATF.as_slice());
    FACTORY.create2(keccak256(salt), INIT[usize::from(tokens[0] == ATF)])
}

pub(super) async fn market(
    ctx: &Ctx,
    f: &Family,
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
        || ctx.dex_addr("dex.factory", f.id)? != FACTORY
        || ctx.dex_addr("dex.quoter", f.id)? != ROUTER
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
        bind(FACTORY, FACTORY_CODE),
        bind(ROUTER, ROUTER_CODE),
        bind(pool, PAIR_CODE[usize::from(edge.0 == ATF || edge.1 == ATF)]),
        roles
    )?;
    let a = batch::decode(&a, IFarm::token0Call {})?;
    let b = batch::decode(&b, IFarm::token1Call {})?;
    let tier = batch::decode(&fee, IFarm::feeCall {})?;
    let embedded = a == ATF;
    if (a, b) != edge && (b, a) != edge
        || b == ATF
        || !embedded && a >= b
        || batch::decode(&owner, IFarm::factoryCall {})? != FACTORY
        || derived([a, b], tier) != pool
    {
        return Err(bad());
    }
    let reg = |x, y| IFarm::getPairCall {
        a: x,
        b: y,
        fee: tier,
    };
    let bal = IFarm::balanceOfCall { account: pool };
    let [ab, ba, router_atf, b0, b1, bfee] = batch::read(
        ctx,
        [
            batch::view(at, FACTORY, reg(a, b)),
            batch::view(at, FACTORY, reg(b, a)),
            batch::view(at, ROUTER, IFarm::antfarmTokenCall {}),
            batch::view(at, a, bal.clone()),
            batch::view(at, b, bal.clone()),
            batch::view(at, ATF, bal.clone()),
        ],
    )
    .await?;
    if batch::decode(&ab, reg(a, b))? != pool
        || batch::decode(&ba, reg(b, a))? != pool
        || batch::decode(&router_atf, IFarm::antfarmTokenCall {})? != ATF
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
        if src != derived([ATF, coin], 10) {
            return Err(bad());
        }
        let [back, registered] = batch::read(
            ctx,
            [
                batch::view(at, src, IFarm::antfarmOracleCall {}),
                batch::view(
                    at,
                    FACTORY,
                    IFarm::getPairCall {
                        a: ATF,
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
                    a: ATF,
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
        tokens: [a, b],
        atf: ATF,
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

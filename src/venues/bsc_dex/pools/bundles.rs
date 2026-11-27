//! Подтверждает заводскую пару Bundles, порядок активов, фактические остатки и источник комиссии BUN.
use super::super::{
    ctx::Ctx,
    math::bundles::{Book, Market},
};
use super::{
    antfarm::IFarm,
    bundles_profile::{self, IBundle, Profile},
    families::Family,
    nami_batch as batch,
};
use crate::gateway::errors::{ErrorCode, GatewayResult};
use alloy::sol_types::SolCall;
use alloy_primitives::{Address, U256};
use serde_json::Value;

pub(crate) async fn module(
    ctx: &Ctx,
    e: &super::registry::Entry,
    target: Address,
    at: &Value,
) -> GatewayResult<()> {
    let bound = batch::read(ctx, [batch::view(at, target, IBundle::routerCall {})]);
    let ((), [encoded]) = tokio::try_join!(
        super::direct_module::verify_code(ctx, e.family.id, target, at),
        bound
    )?;
    if batch::decode(&encoded, IBundle::routerCall {})?
        != ctx.dex_addr("dex.quoter", e.family.id)?
    {
        return Err(ctx.err(
            ErrorCode::InvalidAddress,
            "Bundles Swap: модуль связан с другим роутером",
        ));
    }
    Ok(())
}
pub(crate) async fn official(
    ctx: &Ctx,
    e: &super::registry::Entry,
    book: &Book,
    input: Address,
    n: U256,
    at: &Value,
) -> GatewayResult<U256> {
    let (to, data) = quote_request(ctx, e, book, input, n)?;
    let raw: alloy_primitives::Bytes = super::super::node::rpc::call(
        ctx,
        "eth_call",
        serde_json::json!([{"to":to,"data":data},at]),
    )
    .await?;
    quote_result(book, &raw, n).ok_or_else(|| {
        ctx.err(
            ErrorCode::BadResponse,
            "Bundles Swap: официальный роутер не вернул выход обмена",
        )
    })
}
pub(super) async fn market(
    ctx: &Ctx,
    p: Profile,
    bun: Address,
    pool: Address,
    edge: (Address, Address),
    at: &Value,
) -> GatewayResult<Market> {
    let bad = || {
        ctx.err(
            ErrorCode::NotApplicable,
            "Bundles Swap: регистрация пула или привязка оракула расходится с выбранной парой",
        )
    };
    let [x, y, f, t, r, s, e, o] = batch::read(
        ctx,
        [
            batch::view(at, pool, IFarm::token0Call {}),
            batch::view(at, pool, IFarm::token1Call {}),
            batch::view(at, pool, IFarm::factoryCall {}),
            batch::view(at, pool, IFarm::feeCall {}),
            batch::view(at, pool, IFarm::getReservesCall {}),
            batch::view(at, pool, IFarm::totalSupplyCall {}),
            batch::view(at, pool, IBundle::protocolTokenReserveCall {}),
            batch::view(at, pool, IBundle::pairOracleCall {}),
        ],
    )
    .await?;
    let tokens = [
        batch::decode(&x, IFarm::token0Call {})?,
        batch::decode(&y, IFarm::token1Call {})?,
    ];
    let [a, b] = tokens;
    let tier = batch::decode(&t, IFarm::feeCall {})?;
    if !((a, b) == edge || (b, a) == edge)
        || a == b
        || b == bun
        || (a != bun && a >= b)
        || batch::decode(&f, IFarm::factoryCall {})? != p.factory
        || p.derived(tokens, tier, bun) != pool
    {
        return Err(bad());
    }
    let reg = |a, b| IFarm::getPairCall { a, b, fee: tier };
    let bal = IFarm::balanceOfCall { account: pool };
    let checks = batch::read(
        ctx,
        [
            batch::view(at, p.factory, reg(a, b)),
            batch::view(at, p.factory, reg(b, a)),
            batch::view(at, a, bal.clone()),
            batch::view(at, b, bal.clone()),
            batch::view(at, bun, bal.clone()),
        ],
    );
    let ((), [ab, ba, b0, b1, b2]) = tokio::try_join!(
        super::solidly_classic_factory::runtime(ctx, pool, p.code[usize::from(a == bun)], at),
        checks
    )?;
    if batch::decode(&ab, reg(a, b))? != pool || batch::decode(&ba, reg(b, a))? != pool {
        return Err(bad());
    }
    let addr = batch::decode(&o, IBundle::pairOracleCall {})?;
    let oracle = if a == bun || addr.is_zero() {
        None
    } else {
        let [owner, asset, rate] = batch::read(
            ctx,
            [
                batch::view(at, addr, IFarm::pairCall {}),
                batch::view(at, addr, IFarm::token1Call {}),
                batch::view(at, addr, IFarm::price1AverageCall {}),
            ],
        )
        .await?;
        let asset = batch::decode(&asset, IFarm::token1Call {})?;
        let src = batch::decode(&owner, IFarm::pairCall {})?;
        let side = tokens.iter().position(|&t| t == asset).ok_or_else(bad)?;
        if src != p.derived([bun, asset], 10, bun) {
            return Err(bad());
        }
        let reg = IFarm::getPairCall {
            a: bun,
            b: asset,
            fee: 10,
        };
        let [registered, back] = batch::read(
            ctx,
            [
                batch::view(at, p.factory, reg.clone()),
                batch::view(at, src, IBundle::pairOracleCall {}),
            ],
        )
        .await?;
        if batch::decode(&registered, reg)? != src
            || batch::decode(&back, IBundle::pairOracleCall {})? != addr
        {
            return Err(bad());
        }
        Some((
            side,
            U256::from(batch::decode(&rate, IFarm::price1AverageCall {})?),
        ))
    };
    let res = batch::decode(&r, IFarm::getReservesCall {})?;
    Ok(Market {
        pool,
        tokens,
        tier,
        reserves: [res.x.to(), res.y.to()],
        balances: [
            batch::decode(&b0, bal.clone())?,
            batch::decode(&b1, bal.clone())?,
            batch::decode(&b2, bal)?,
        ],
        fee_reserve: batch::decode(&e, IBundle::protocolTokenReserveCall {})?,
        supply: batch::decode(&s, IFarm::totalSupplyCall {})?,
        oracle,
    })
}
pub(crate) async fn book(
    ctx: &Ctx,
    f: &Family,
    pool: Address,
    edge: (Address, Address),
    at: &Value,
) -> GatewayResult<Book> {
    if ctx.chain_id() != Some(1) {
        return Err(ctx.err(
            ErrorCode::NotApplicable,
            "Bundles Swap: ожидается Ethereum mainnet",
        ));
    }
    let revision = ctx.settings.generation();
    let p = Profile::read(ctx, f)?;
    let shared = bundles_profile::common(ctx, p, at).await?;
    let m = if pool == shared.fee_pair.pool
        && (shared.fee_pair.tokens == [edge.0, edge.1]
            || shared.fee_pair.tokens == [edge.1, edge.0])
    {
        shared.fee_pair.clone()
    } else {
        market(ctx, p, shared.bun, pool, edge, at).await?
    };
    if ctx.settings.generation() != revision {
        return Err(ctx.err(
            ErrorCode::StaleData,
            "Bundles Swap: профиль изменился во время чтения",
        ));
    }
    Ok(Book {
        market: m,
        fee_pair: shared.fee_pair,
        bun: shared.bun,
        weth: p.weth,
        fee_rate_after: shared.fee_rate_after,
        router_balance: shared.wallet_balance,
        deposits: shared.deposits,
    })
}
pub(super) async fn verify(ctx: &Ctx, c: &super::verify::Candidate) -> super::verify::Verdict {
    use super::verify::Verdict;
    let Ok(pool) = c.pool.parse::<Address>() else {
        return Verdict::Rejected("Bundles Swap: неверный адрес".into());
    };
    let read = async {
        let at = super::nami::block(ctx, "latest").await?;
        book(ctx, c.family, pool, (c.coin, c.quote), &at).await
    };
    match read.await {
        Ok(book) => {
            let mut found =
                super::verify_woo::found(c, crate::core::types::PoolRef::Contract(pool));
            [found.token0, found.token1] = book.market.tokens;
            Verdict::Taken(Box::new(found))
        }
        Err(e) if e.code == ErrorCode::NotApplicable => Verdict::Rejected(e.message),
        Err(e) => Verdict::Failed(e.message),
    }
}

pub(crate) fn quote_request(
    ctx: &Ctx,
    e: &super::registry::Entry,
    book: &Book,
    input: Address,
    amount: U256,
) -> GatewayResult<(Address, alloy_primitives::Bytes)> {
    let p = Profile::read(ctx, e.family)?;
    let m = &book.market;
    let i = m
        .tokens
        .iter()
        .position(|&x| x == input)
        .ok_or_else(|| ctx.err(ErrorCode::InvalidAddress, "Bundles Swap: неизвестный вход"))?;
    let out = m.tokens[1 - i];
    let raw = if m.tokens[0] == book.bun {
        IBundle::getAmountsOutCall {
            amount,
            path: vec![input, out],
            fees: vec![m.tier],
        }
        .abi_encode()
    } else if input == book.weth {
        IBundle::estimateExactETHForTokensCall {
            output: out,
            fee: m.tier,
            amount,
        }
        .abi_encode()
    } else if out == book.weth {
        IBundle::estimateExactTokensForETHCall {
            input,
            fee: m.tier,
            amount,
        }
        .abi_encode()
    } else {
        return Err(ctx.err(
            ErrorCode::NoRoute,
            "Bundles Swap: нужен бюджет отдельной комиссии BUN",
        ));
    };
    Ok((p.router, raw.into()))
}
pub(crate) fn quote_result(book: &Book, raw: &[u8], amount: U256) -> Option<U256> {
    if book.market.tokens[0] == book.bun {
        let out = IBundle::getAmountsOutCall::abi_decode_returns(raw).ok()?;
        match out.as_slice() {
            [start, paid] if *start == amount => Some(*paid),
            _ => None,
        }
    } else {
        IBundle::estimateExactTokensForETHCall::abi_decode_returns(raw).ok()
    }
}

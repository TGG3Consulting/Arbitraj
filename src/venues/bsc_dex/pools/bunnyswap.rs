//! Единственный рынок RabbitRouter. Проверяются оба контракта, связывающие адреса,
//! каноническая обёртка, неизменяемость ставок после отказа владельцев от управления.
use super::{
    abi_check::IPoolImmutables as P, abi_v2v3::IV2Pair as Pair, discover::Found, families::Family,
    nami_batch as batch, registry::Entry, verify::Candidate,
};
use crate::{
    core::types::{PoolRef, PoolState},
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::{ctx::Ctx, node::multicall},
};
use alloy::{sol, sol_types::SolCall};
use alloy_primitives::{Address, B256, Bytes, U256, keccak256};
use serde_json::{Value, json};

sol! {
    interface IRabbit {
        function pair() external view returns(address);
        function router() external view returns(address);
        function isOpen(bytes4 selector) external view returns(bool);
        function owner() external view returns(address);
        function feeTier() external view returns(uint64);
        function protocolFee() external view returns(uint64);
        function isAllowedRouter(address market) external view returns(bool);
        function getAmountOut(uint256 input,uint256 reserveIn,uint256 reserveOut,uint64 fee) external pure returns(uint256);
    }
}

pub(crate) fn tokens(ctx: &Ctx, f: &Family) -> GatewayResult<(Address, Address)> {
    Ok((
        ctx.dex_addr("dex.bunny_token", f.id)?,
        ctx.addr("dex.native_wrapped")?,
    ))
}

pub(crate) async fn market(
    ctx: &Ctx,
    f: &Family,
    address: Address,
    pair: (Address, Address),
    at: &Value,
) -> GatewayResult<u32> {
    let router = ctx.dex_addr("dex.quoter", f.id)?;
    let expected = tokens(ctx, f)?;
    let invalid = || {
        ctx.err(
            ErrorCode::QuoteMismatch,
            "RabbitRouter: не совпали адрес рынка, токены, разрешение роутера, код или комиссии",
        )
    };
    if address != ctx.dex_addr("dex.bunny_pair", f.id)?
        || pair != expected
        || expected.0 >= expected.1
    {
        return Err(invalid());
    }
    let hash =
        |key| -> GatewayResult<B256> { ctx.dex_text(key, f.id)?.parse().map_err(|_| invalid()) };
    let [
        pc,
        rc,
        lp,
        protocol,
        allowed,
        owner,
        admin,
        registered,
        a,
        b,
        c,
        d,
    ] = batch::read(
        ctx,
        [
            ("eth_getCode", json!([address, at])),
            ("eth_getCode", json!([router, at])),
            batch::view(at, address, IRabbit::feeTierCall {}),
            batch::view(at, router, IRabbit::protocolFeeCall {}),
            batch::view(at, address, IRabbit::isAllowedRouterCall { market: router }),
            batch::view(at, address, IRabbit::ownerCall {}),
            batch::view(at, router, IRabbit::ownerCall {}),
            batch::view(at, router, IRabbit::pairCall {}),
            batch::view(at, address, P::token0Call {}),
            batch::view(at, address, P::token1Call {}),
            batch::view(at, router, P::token0Call {}),
            batch::view(at, router, P::token1Call {}),
        ],
    )
    .await?;
    if pc.is_empty()
        || rc.is_empty()
        || keccak256(&pc) != hash("dex.pair_runtime_hash")?
        || keccak256(&rc) != hash("dex.quoter_runtime_hash")?
        || batch::decode(&protocol, IRabbit::protocolFeeCall {})? != 0
        || !batch::decode(&allowed, IRabbit::isAllowedRouterCall { market: router })?
        || !batch::decode(&owner, IRabbit::ownerCall {})?.is_zero()
        || !batch::decode(&admin, IRabbit::ownerCall {})?.is_zero()
        || batch::decode(&registered, IRabbit::pairCall {})? != address
        || (
            batch::decode(&a, P::token0Call {})?,
            batch::decode(&b, P::token1Call {})?,
        ) != expected
        || (
            batch::decode(&c, P::token0Call {})?,
            batch::decode(&d, P::token1Call {})?,
        ) != expected
    {
        return Err(invalid());
    }
    let fee = batch::decode(&lp, IRabbit::feeTierCall {})?;
    if fee >= 1000 {
        return Err(invalid());
    }
    Ok((fee as u32) * 1000)
}

pub(crate) async fn verify(ctx: &Ctx, c: &Candidate, pool: Address) -> GatewayResult<Found> {
    let pair = super::discover::sorted(c.coin, c.quote);
    let at = super::nami::block(ctx, "latest").await?;
    let rate = market(ctx, c.family, pool, pair, &at).await?;
    let mut entry = super::verify_woo::found(c, PoolRef::Contract(pool));
    entry.fee_pips = Some(rate);
    Ok(entry)
}

pub(crate) async fn fresh(ctx: &Ctx, e: &Entry, at: &Value) -> GatewayResult<()> {
    let rate = market(
        ctx,
        e.family,
        e.pool.reference.address(),
        (e.pool.token0, e.pool.token1),
        at,
    )
    .await?;
    if e.pool.fee_pips != Some(rate) {
        return Err(ctx.err(
            ErrorCode::QuoteMismatch,
            "RabbitRouter: комиссия котировки устарела",
        ));
    }
    Ok(())
}

pub(crate) fn quote(
    ctx: &Ctx,
    e: &Entry,
    input: Address,
    amount: U256,
) -> GatewayResult<(Address, Bytes)> {
    let invalid = || {
        ctx.err(
            ErrorCode::NoRoute,
            "RabbitRouter: неизвестна сторона, комиссия или резерв",
        )
    };
    let PoolState::V2 { reserve0, reserve1 } = &e.pool.state else {
        return Err(invalid());
    };
    let (rin, rout) = if input == e.pool.token0 {
        (*reserve0, *reserve1)
    } else if input == e.pool.token1 {
        (*reserve1, *reserve0)
    } else {
        return Err(invalid());
    };
    let fee = e
        .pool
        .fee_pips
        .filter(|f| *f < 1_000_000 && f % 1000 == 0)
        .ok_or_else(invalid)?
        / 1000;
    let router = ctx.dex_addr("dex.quoter", e.family.id)?;
    let calls = [
        (
            e.pool.reference.address(),
            Bytes::from(Pair::getReservesCall {}.abi_encode()),
        ),
        (
            router,
            Bytes::from(
                IRabbit::getAmountOutCall {
                    input: amount,
                    reserveIn: rin,
                    reserveOut: rout,
                    fee: u64::from(fee),
                }
                .abi_encode(),
            ),
        ),
    ];
    Ok((ctx.addr("dex.multicall3")?, multicall::encode(&calls)))
}

pub(crate) fn decode(e: &Entry, raw: &[u8]) -> Option<U256> {
    let PoolState::V2 { reserve0, reserve1 } = &e.pool.state else {
        return None;
    };
    let mut rows = multicall::decode(raw, 2).ok()?.into_iter();
    let onchain = Pair::getReservesCall::abi_decode_returns(&rows.next()?.ok()?).ok()?;
    if *reserve0 != U256::from(onchain.reserve0) || *reserve1 != U256::from(onchain.reserve1) {
        return None;
    }
    IRabbit::getAmountOutCall::abi_decode_returns(&rows.next()?.ok()?).ok()
}

/// Штатный роутер проверяет минимум FRIEND до перевода. Подтверждаем точный перевод в неизменяемом коде Points.
pub(crate) async fn native_state(ctx: &Ctx, e: &Entry, at: &Value) -> GatewayResult<PoolState> {
    fresh(ctx, e, at).await?;
    let (token, _) = tokens(ctx, e.family)?;
    let fail = || {
        ctx.err(
            ErrorCode::QuoteMismatch,
            "RabbitRouter: код FRIEND или разрешение передачи не подтверждено",
        )
    };
    if ctx.chain_id() != Some(8453)
        || token != alloy_primitives::address!("0bd4887f7d41b35cd75dff9ffee2856106f86670")
    {
        return Err(fail());
    }
    let opened = |s: &[u8]| IRabbit::isOpenCall {
        selector: alloy_primitives::FixedBytes::<4>::from_slice(&keccak256(s)[..4]),
    };
    let open = [
        opened(b"transfer(address,uint256)"),
        opened(b"transferFrom(address,address,uint256)"),
        opened(b"approve(address,uint256)"),
    ];
    let pool = e.pool.reference.address();
    let [code, admin, t, tf, a, r] = batch::read(
        ctx,
        [
            ("eth_getCode", json!([token, at])),
            batch::view(at, token, IRabbit::ownerCall {}),
            batch::view(at, token, open[0].clone()),
            batch::view(at, token, open[1].clone()),
            batch::view(at, token, open[2].clone()),
            batch::view(at, pool, Pair::getReservesCall {}),
        ],
    )
    .await?;
    if keccak256(&code)
        != alloy_primitives::b256!(
            "358aa3e7ee950f58863c601550d37477d36b1dd76c1ce15342ac6649acf9caa8"
        )
        || !batch::decode(&admin, IRabbit::ownerCall {})?.is_zero()
    {
        return Err(fail());
    }
    for (reply, call) in [t, tf, a].into_iter().zip(open) {
        if !batch::decode(&reply, call)? {
            return Err(fail());
        }
    }
    let r = batch::decode(&r, Pair::getReservesCall {})?;
    Ok(PoolState::V2 {
        reserve0: U256::from(r.reserve0),
        reserve1: U256::from(r.reserve1),
    })
}

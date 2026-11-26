//! Снимок пары Memebox на одном каноническом блоке: резерв, доступный остаток, физические токены и тариф.
//! Используют загрузчик, независимый квотер и проверка перед исполнением.
use super::{
    abi_check::IPoolImmutables as IP,
    abi_v2v3::{IV2Factory as Factory, IV2Pair as Pair},
    families::Family,
    load::Loaded,
    nami_batch as batch,
    registry::Entry,
};
use crate::{
    core::types::{MemeboxState, PoolState},
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::{ctx::Ctx, node::abi::IERC20},
};
use alloy::{sol, sol_types::SolCall};
use alloy_primitives::{Address, B256, Bytes, U256};
use serde_json::Value;

sol! {
    interface IMemebox {
        function balances() external view returns (uint256 a,uint256 b);
        function poolFee() external view returns (uint16);
        function protocolRatio() external view returns (uint16);
        function INIT_CODE_HASH() external view returns(bytes32);
        function getAmountOut(address a,address b,uint256 credit) external view returns(uint256);
    }
}

async fn state(
    ctx: &Ctx,
    f: &Family,
    pool: Address,
    (a, b): (Address, Address),
    at: &Value,
) -> GatewayResult<MemeboxState> {
    let generation = ctx.settings.generation();
    let bad = || {
        ctx.err(
            ErrorCode::NotApplicable,
            "Memebox: происхождение, токены, комиссия или обеспечение пары не подтверждены",
        )
    };
    if a >= b
        || a.is_zero()
        || ctx.dex_text("dex.v2_fee_source", f.id)? != "pool"
        || ctx.dex_text("dex.v2_fee_method", f.id)? != "poolFee()"
        || ctx.dex_int("dex.v2_fee_denominator", f.id)? != 10_000
    {
        return Err(bad());
    }
    let factory = ctx.dex_addr("dex.factory", f.id)?;
    let runtime = ctx
        .dex_text("dex.factory_runtime_hash", f.id)?
        .parse::<B256>()
        .map_err(|_| bad())?;
    super::solidly_classic_factory::plain(ctx, factory, runtime, at).await?;
    let hashes = super::verify_algebra_dir::hashes_for(ctx, f.id).map_err(|_| bad())?;
    let reserves = Pair::getReservesCall {};
    let available = IMemebox::balancesCall {};
    let coin = IERC20::balanceOfCall { owner: pool };
    let one = Factory::getPairCall {
        tokenA: a,
        tokenB: b,
    };
    let two = Factory::getPairCall {
        tokenA: b,
        tokenB: a,
    };
    let creator = IP::factoryCall {};
    let first = IP::token0Call {};
    let second = IP::token1Call {};
    let init = IMemebox::INIT_CODE_HASHCall {};
    let fee = IMemebox::poolFeeCall {};
    let ratio = IMemebox::protocolRatioCall {};
    let [
        r,
        v,
        left,
        right,
        listed,
        reverse,
        origin,
        t0,
        t1,
        code,
        rate,
        split,
    ] = batch::read(
        ctx,
        [
            batch::view(at, pool, reserves.clone()),
            batch::view(at, pool, available.clone()),
            batch::view(at, a, coin.clone()),
            batch::view(at, b, coin.clone()),
            batch::view(at, factory, one.clone()),
            batch::view(at, factory, two.clone()),
            batch::view(at, pool, creator.clone()),
            batch::view(at, pool, first.clone()),
            batch::view(at, pool, second.clone()),
            batch::view(at, factory, init.clone()),
            batch::view(at, pool, fee.clone()),
            batch::view(at, pool, ratio.clone()),
        ],
    )
    .await?;
    let hash = batch::decode(&code, init)?;
    let r = batch::decode(&r, reserves)?;
    let v = batch::decode(&v, available)?;
    let market = MemeboxState {
        reserves: [U256::from(r.reserve0), U256::from(r.reserve1)],
        available: [v.a, v.b],
        physical: [
            batch::decode(&left, coin.clone())?,
            batch::decode(&right, coin)?,
        ],
        fee_bps: batch::decode(&rate, fee)?,
        protocol_bps: batch::decode(&split, ratio)?,
    };
    if generation != ctx.settings.generation()
        || !hashes.contains(&hash)
        || super::verify::v2_create2(factory, a, b, hash) != pool
        || batch::decode(&origin, creator)? != factory
        || batch::decode(&listed, one)? != pool
        || batch::decode(&reverse, two)? != pool
        || batch::decode(&t0, first)? != a
        || batch::decode(&t1, second)? != b
        || market.fee_bps > 10_000
        || market.protocol_bps > 10_000
        || market
            .available
            .iter()
            .zip(market.physical)
            .any(|(v, p)| *v > p)
    {
        return Err(bad());
    }
    Ok(market)
}

pub(super) async fn load(
    ctx: &Ctx,
    f: &Family,
    pool: Address,
    pair: (Address, Address),
    block: &str,
) -> GatewayResult<Loaded> {
    let at = super::elfomo::block(ctx, block).await?;
    let s = state(ctx, f, pool, pair, &at).await?;
    Ok(Loaded {
        fee_pips: Some(u32::from(s.fee_bps) * 100),
        state: PoolState::Memebox(Box::new(s)),
        ticks: None,
        bins: None,
    })
}

pub(crate) async fn refresh(
    ctx: &Ctx,
    e: &mut Entry,
    h: super::super::quote::white_state::Head,
) -> GatewayResult<()> {
    let at = h.selector();
    let pool = super::direct_module::market(ctx, e)?;
    let s = state(ctx, e.family, pool, (e.pool.token0, e.pool.token1), &at).await?;
    super::direct_module::verify_factory(ctx, e.family, ctx.dex_addr("dex.router", e.family.id)?, &at).await?;
    e.pool.fee_pips = Some(u32::from(s.fee_bps) * 100);
    e.pool.state = PoolState::Memebox(Box::new(s));
    e.pool.block = h.number.to();
    Ok(())
}

pub(crate) async fn fresh(ctx: &Ctx, e: &Entry, at: &Value) -> GatewayResult<()> {
    let pool = super::direct_module::market(ctx, e)?;
    let expected = state(ctx, e.family, pool, (e.pool.token0, e.pool.token1), at).await?;
    if let PoolState::Memebox(s) = &e.pool.state
        && **s == expected
    {
        return super::direct_module::verify_factory(ctx, e.family, ctx.dex_addr("dex.router", e.family.id)?, at).await;
    }
    Err(ctx.err(
        ErrorCode::StaleData,
        "Memebox: состояние или тариф изменились к проверке пути",
    ))
}

pub(crate) fn calldata(a: Address, b: Address, credit: U256) -> Bytes {
    IMemebox::getAmountOutCall { a, b, credit }
        .abi_encode()
        .into()
}

pub(crate) fn decode(raw: &[u8]) -> Option<U256> {
    (raw.len() == 32).then(|| U256::from_be_slice(raw))
}

pub(crate) async fn quote(
    ctx: &Ctx,
    e: &Entry,
    input: Address,
    amount: U256,
    block: &str,
) -> GatewayResult<U256> {
    let at = super::elfomo::block(ctx, block).await?;
    let module_address = ctx.dex_addr("dex.router", e.family.id)?;
    super::direct_module::verify_factory(ctx, e.family, module_address, &at).await?;
    let output = if input == e.pool.token0 {
        e.pool.token1
    } else if input == e.pool.token1 {
        e.pool.token0
    } else {
        return Err(ctx.err(
            ErrorCode::InvalidAddress,
            "Memebox: вход отсутствует в паре",
        ));
    };
    let req = IMemebox::getAmountOutCall {
        a: input,
        b: output,
        credit: amount,
    };
    let [raw] = batch::read(ctx, [batch::view(&at, module_address, req.clone())]).await?;
    batch::decode(&raw, req)
}

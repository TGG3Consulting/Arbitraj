//! Снимок spot, долгов и физических остатков. Загрузчик и независимый контрольный квотер берут один блок.
use super::{
    ammalgam::{IAmmalgam, bad, market},
    families::Family,
    nami_batch as batch,
};
use crate::{
    core::types::AmmalgamState,
    gateway::errors::GatewayResult,
    venues::bsc_dex::{ctx::Ctx, node::abi::IERC20, quote::white_state},
};
use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256};
/// TokenController.getReserves and Pair.swap use INTEREST_PERIOD_FOR_SWAP.
/// https://repo.sourcify.dev/1/0x5C0d819609Eee2cdCf326f0e898A8447e06a45Be
const SWAP_INTEREST_PERIOD: u32 = 24 * 60 * 60 - 1;
pub(super) async fn state(
    ctx: &Ctx,
    f: &Family,
    pool: Address,
    pair: (Address, Address),
    tag: &str,
) -> GatewayResult<AmmalgamState> {
    let head = white_state::head(ctx, tag).await?;
    state_at(ctx, f, pool, pair, head).await
}
pub(crate) async fn state_at(
    ctx: &Ctx,
    f: &Family,
    pool: Address,
    pair: (Address, Address),
    head: white_state::Head,
) -> GatewayResult<AmmalgamState> {
    let generation = ctx.settings.generation();
    let at = head.selector();
    let bal = IERC20::balanceOfCall { owner: pool };
    let req = [
        batch::view(&at, pool, IAmmalgam::getReservesCall {}),
        batch::view(&at, pool, IAmmalgam::referenceReservesCall {}),
        batch::view(&at, pair.0, bal.clone()),
        batch::view(&at, pair.1, bal.clone()),
    ];
    let (coins, [raw, refs, cash0, cash1]) =
        tokio::try_join!(market(ctx, f, pool, pair, &at), batch::read(ctx, req))?;
    let reserves = batch::decode(&raw, IAmmalgam::getReservesCall {})?;
    let reference = batch::decode(&refs, IAmmalgam::referenceReservesCall {})?;
    // TokenController.getReserves and AmmalgamPair.swap apply the same interest window.
    let timestamp = head.timestamp.to::<u64>() as u32;
    let interest = timestamp.wrapping_sub(reserves.lastLending) >= SWAP_INTEREST_PERIOD;
    let q = IAmmalgam::totalAssetsAndSharesCall {
        withInterest: interest,
    };
    let [raw] = batch::read(ctx, [batch::view(&at, pool, q.clone())]).await?;
    let assets = batch::decode(&raw, q)?.assets;
    let order = |vals: [U256; 2]| {
        if coins[0] == pair.0 {
            vals
        } else {
            [vals[1], vals[0]]
        }
    };
    if generation != ctx.settings.generation() {
        return Err(bad(ctx));
    }
    Ok(AmmalgamState {
        reserves: order([reserves.x.to::<U256>(), reserves.y.to::<U256>()]),
        reference: order([reference.x.to::<U256>(), reference.y.to::<U256>()]),
        missing: order([
            assets[4].saturating_sub(assets[1]).to::<U256>(),
            assets[5].saturating_sub(assets[2]).to::<U256>(),
        ]),
        cash: [
            batch::decode(&cash0, bal.clone())?,
            batch::decode(&cash1, bal)?,
        ],
    })
}

/// Только запрос контрольному квотеру. Объём планировщика считается локально по тому же блоку.
pub(crate) fn request(
    ctx: &Ctx,
    e: &super::registry::Entry,
    input: Address,
    amount: U256,
) -> GatewayResult<(Address, Bytes)> {
    let crate::core::types::PoolState::Ammalgam(s) = &e.pool.state else {
        return Err(bad(ctx));
    };
    let i = if input == e.pool.token0 {
        0
    } else if input == e.pool.token1 {
        1
    } else {
        return Err(bad(ctx));
    };
    let call = IAmmalgam::computeExpectedSwapAmountOutCall {
        input: amount,
        reserveIn: s.reserves[i],
        referenceIn: s.reference[i],
        reserveOut: s.reserves[1 - i],
        missingIn: s.missing[i],
        missingOut: s.missing[1 - i],
    };
    Ok((
        ctx.dex_addr("dex.quoter", e.family.id)?,
        call.abi_encode().into(),
    ))
}
pub(crate) fn payout(_: &super::registry::Entry, _: Address, _: U256, raw: &[u8]) -> Option<U256> {
    IAmmalgam::computeExpectedSwapAmountOutCall::abi_decode_returns_validate(raw).ok()
}

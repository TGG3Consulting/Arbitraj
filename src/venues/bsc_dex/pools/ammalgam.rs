//! Канонический spot-рынок dLEx. У пула внешние резервы для обмена, долги и физический ERC20 различны.
use super::{
    families::Family,
    nami_batch as batch,
    verify::{Candidate, Verdict},
};
use crate::{
    core::types::{AmmalgamState, PoolRef},
    gateway::errors::{ErrorCode, GatewayError, GatewayResult},
    venues::bsc_dex::{ctx::Ctx, node::abi::IERC20, quote::white_state},
};
use alloy::sol_types::SolCall;
use alloy_primitives::{Address, B256, Bytes, U256, b256, keccak256};
use serde_json::{Value, json};

alloy::sol! {interface IAmmalgam {
    function computeExpectedSwapAmountOut(uint256 input,uint256 reserveIn,uint256 referenceIn,uint256 reserveOut,uint256 missingIn,uint256 missingOut) external pure returns(uint256);
    function getPair(address a,address b) external view returns(address);
    function pairBeacon() external view returns(address);
    function implementation() external view returns(address);
    function underlyingTokens() external view returns(address x,address y);
    function getReserves() external view returns(uint112 x,uint112 y,uint32 lastLending);
    function referenceReserves() external view returns(uint112 x,uint112 y);
    function totalAssetsAndShares(bool withInterest) external view returns(uint112[6] assets,uint112[6] shares);
}}
/// TokenController.INTEREST_PERIOD_FOR_SWAP.
const SWAP_INTEREST_PERIOD: u32 = 24 * 60 * 60 - 1;
const BEACON_SLOT: B256 = b256!("a3f0ad74e5423aebfd80d3ef4346578335a9a72aeaee59ff6cb3582b35133d50");
fn bad(ctx: &Ctx) -> GatewayError {
    ctx.err(
        ErrorCode::NotApplicable,
        "Ammalgam: состав пары, код или реализация в регистре не совпали с профилем",
    )
}
fn slot_address(raw: &[u8]) -> Option<Address> {
    (raw.len() == 32 && raw[..12].iter().all(|x| *x == 0)).then(|| Address::from_slice(&raw[12..]))
}
pub(crate) async fn market(
    ctx: &Ctx,
    f: &Family,
    pool: Address,
    pair: (Address, Address),
    at: &Value,
) -> GatewayResult<[Address; 2]> {
    let gen_at_start = ctx.settings.generation();
    let id = f.id;
    let factory = ctx.dex_addr("dex.factory", id)?;
    let helper = ctx.dex_addr("dex.quoter", id)?;
    let roles = ctx.dex_list("dex.proxy_implementations", id)?;
    let hashes = ctx.dex_list("dex.proxy_runtime_hashes", id)?;
    let ([beacon, implementation], [beacon_hash, impl_hash]) =
        (roles.as_slice(), hashes.as_slice())
    else {
        return Err(bad(ctx));
    };
    let beacon: Address = beacon.parse().map_err(|_| bad(ctx))?;
    let implementation: Address = implementation.parse().map_err(|_| bad(ctx))?;
    let beacon_hash: B256 = beacon_hash.parse().map_err(|_| bad(ctx))?;
    let impl_hash: B256 = impl_hash.parse().map_err(|_| bad(ctx))?;
    if at["requireCanonical"] != true
        || at["blockHash"].as_str().is_none()
        || [
            pool,
            beacon,
            implementation,
            factory,
            helper,
            pair.0,
            pair.1,
        ]
        .contains(&Address::ZERO)
        || pair.0 == pair.1
    {
        return Err(bad(ctx));
    }
    let lookup = IAmmalgam::getPairCall {
        a: pair.0,
        b: pair.1,
    };
    let reverse = IAmmalgam::getPairCall {
        a: pair.1,
        b: pair.0,
    };
    let req = [
        batch::view(at, factory, lookup.clone()),
        batch::view(at, factory, reverse.clone()),
        batch::view(at, pool, IAmmalgam::underlyingTokensCall {}),
        batch::view(at, factory, IAmmalgam::pairBeaconCall {}),
        batch::view(at, beacon, IAmmalgam::implementationCall {}),
        ("eth_getStorageAt", json!([pool, BEACON_SLOT, at])),
        (
            "eth_getStorageAt",
            json!([pool, super::abi_woo::implementation_slot(), at]),
        ),
        ("eth_getCode", json!([beacon, at])),
        ("eth_getCode", json!([implementation, at])),
        ("eth_getCode", json!([helper, at])),
    ];
    let (
        (),
        [
            ab,
            ba,
            coins,
            owner,
            current,
            beacon_slot,
            impl_slot,
            bcode,
            icode,
            hcode,
        ],
    ) = tokio::try_join!(
        super::v2_pair_binding::code(ctx, id, pool, at),
        batch::read(ctx, req)
    )?;
    let coins = batch::decode(&coins, IAmmalgam::underlyingTokensCall {})?;
    if batch::decode(&ab, lookup)? != pool
        || batch::decode(&ba, reverse)? != pool
        || super::discover::sorted(coins.x, coins.y) != super::discover::sorted(pair.0, pair.1)
        || batch::decode(&owner, IAmmalgam::pairBeaconCall {})? != beacon
        || slot_address(&beacon_slot) != Some(beacon)
        || batch::decode(&current, IAmmalgam::implementationCall {})? != implementation
        || slot_address(&impl_slot) != Some(implementation)
        || bcode.is_empty()
        || icode.is_empty()
        || hcode.is_empty()
        || keccak256(bcode) != beacon_hash
        || keccak256(icode) != impl_hash
        || keccak256(hcode) != super::v2_pair_binding::expected(ctx, id, "dex.quoter_runtime_hash")?
        || gen_at_start != ctx.settings.generation()
    {
        return Err(bad(ctx));
    }
    Ok([coins.x, coins.y])
}
pub(super) async fn verify(ctx: &Ctx, c: &Candidate) -> Verdict {
    let Ok(pool) = c.pool.parse() else {
        return Verdict::Rejected("Ammalgam: неверный адрес пула".into());
    };
    let result: GatewayResult<_> = async {
        let head = white_state::head(ctx, "latest").await?;
        market(ctx, c.family, pool, (c.coin, c.quote), &head.selector()).await?;
        Ok(super::verify_woo::found(c, PoolRef::Contract(pool)))
    }
    .await;
    match result {
        Ok(found) => Verdict::Taken(Box::new(found)),
        Err(e) if e.code == ErrorCode::NotApplicable => Verdict::Rejected(e.message),
        Err(e) => Verdict::Failed(e.message),
    }
}
pub(super) async fn state(
    ctx: &Ctx,
    f: &Family,
    pool: Address,
    pair: (Address, Address),
    tag: &str,
) -> GatewayResult<AmmalgamState> {
    let generation = ctx.settings.generation();
    let head = white_state::head(ctx, tag).await?;
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
        reserves: order([reserves.x.into(), reserves.y.into()]),
        reference: order([reference.x.into(), reference.y.into()]),
        missing: order([
            assets[4].saturating_sub(assets[1]).into(),
            assets[5].saturating_sub(assets[2]).into(),
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

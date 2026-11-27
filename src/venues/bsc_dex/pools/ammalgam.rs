//! Канонический spot-рынок dLEx. У пула внешние резервы для обмена, долги и физический ERC20 различны.
use super::{
    families::Family,
    nami_batch as batch,
    verify::{Candidate, Verdict},
};
use crate::{
    core::types::PoolRef,
    gateway::errors::{ErrorCode, GatewayError, GatewayResult},
    venues::bsc_dex::{ctx::Ctx, quote::white_state},
};
use alloy_primitives::{Address, B256, b256, keccak256};
use serde_json::{Value, json};

alloy::sol! {interface IAmmalgam {
    function computeExpectedSwapAmountOut(uint256 input,uint256 reserveIn,uint256 referenceIn,uint256 reserveOut,uint256 missingIn,uint256 missingOut) external pure returns(uint256);
    function factory() external view returns(address);
    function helper() external view returns(address);
    function getPair(address a,address b) external view returns(address);
    function pairBeacon() external view returns(address);
    function implementation() external view returns(address);
    function underlyingTokens() external view returns(address x,address y);
    function getReserves() external view returns(uint112 x,uint112 y,uint32 lastLending);
    function referenceReserves() external view returns(uint112 x,uint112 y);
    function totalAssetsAndShares(bool withInterest) external view returns(uint112[6] assets,uint112[6] shares);
}}
/// ERC-1967 beacon storage slot: https://eips.ethereum.org/EIPS/eip-1967#beacon-contract-address
const BEACON_SLOT: B256 = b256!("a3f0ad74e5423aebfd80d3ef4346578335a9a72aeaee59ff6cb3582b35133d50");
pub(super) fn bad(ctx: &Ctx) -> GatewayError {
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
    if ctx.chain_id() != Some(crate::settings::evm_profiles::ETHEREUM_MAINNET) {
        return Err(bad(ctx));
    }
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
pub(super) use super::ammalgam_state::state;
pub(crate) use super::ammalgam_state::{payout, request};

pub(crate) use super::ammalgam_step::{check_step, graph_target};

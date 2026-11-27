//! Pinned linked-list order book of the verified Rubicon Classic proxy.
use super::{
    families::Family,
    nami_batch as batch,
    verify::{Candidate, Verdict},
};
use crate::{
    core::types::{
        PoolRef,
        rubicon::{Book, Offer},
    },
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::ctx::Ctx,
};
use alloy::sol;
use alloy_primitives::{Address, U256};
use serde_json::Value;
use std::collections::HashSet;

sol! { interface IRubiconMarket {
    function getBestOffer(address sellToken,address buyToken) external view returns(uint256);
    function getWorseOffer(uint256 id) external view returns(uint256);
    function getOffer(uint256 id) external view returns(uint256 available,address sellToken,uint256 requested,address buyToken);
    function getFeeBPS() external view returns(uint256);
    function makerFee() external view returns(uint256);
    function getMinSell(address token) external view returns(uint256);
    function isClosed() external view returns(bool);
    function buyEnabled() external view returns(bool);
    function matchingEnabled() external view returns(bool);
} }

fn bad(ctx: &Ctx) -> crate::gateway::errors::GatewayError {
    ctx.err(
        ErrorCode::BadResponse,
        "Rubicon Classic: реквизиты книги, порядок заявок или комиссия изменились",
    )
}

async fn side(
    ctx: &Ctx,
    market: Address,
    tokens: [Address; 2],
    i: usize,
    at: &Value,
) -> GatewayResult<Vec<Offer>> {
    let head = IRubiconMarket::getBestOfferCall {
        sellToken: tokens[1 - i],
        buyToken: tokens[i],
    };
    let [encoded] = batch::read(ctx, [batch::view(at, market, head.clone())]).await?;
    let mut id = batch::decode(&encoded, head)?;
    let mut visited = HashSet::new();
    let mut offers = Vec::new();
    while !id.is_zero() {
        if !visited.insert(id) {
            return Err(bad(ctx));
        }
        let info = IRubiconMarket::getOfferCall { id };
        let worse = IRubiconMarket::getWorseOfferCall { id };
        let [raw, next] = batch::read(
            ctx,
            [
                batch::view(at, market, info.clone()),
                batch::view(at, market, worse.clone()),
            ],
        )
        .await?;
        let terms = batch::decode(&raw, info)?;
        if terms.available.is_zero()
            || terms.requested.is_zero()
            || (terms.sellToken, terms.buyToken) != (tokens[1 - i], tokens[i])
        {
            return Err(bad(ctx));
        }
        offers.push(Offer {
            id,
            available: terms.available.try_into().map_err(|_| bad(ctx))?,
            requested: terms.requested.try_into().map_err(|_| bad(ctx))?,
        });
        id = batch::decode(&next, worse)?;
    }
    Ok(offers)
}

pub(crate) async fn load(
    ctx: &Ctx,
    f: &Family,
    ref_addr: Address,
    (a, b): (Address, Address),
    at: &Value,
) -> GatewayResult<Book> {
    let epoch = ctx.settings.generation();
    let market = super::solidly_classic_factory::at(ctx, f, at).await?;
    if market != ref_addr
        || a.is_zero()
        || a >= b
        || ctx.dex_list("dex.proxy_implementations", f.id)?.len() != 1
    {
        return Err(bad(ctx));
    }
    let dust_a = IRubiconMarket::getMinSellCall { token: a };
    let dust_b = IRubiconMarket::getMinSellCall { token: b };
    let [closed, enabled, matching, trader, maker, da, db] = batch::read(
        ctx,
        [
            batch::view(at, market, IRubiconMarket::isClosedCall {}),
            batch::view(at, market, IRubiconMarket::buyEnabledCall {}),
            batch::view(at, market, IRubiconMarket::matchingEnabledCall {}),
            batch::view(at, market, IRubiconMarket::getFeeBPSCall {}),
            batch::view(at, market, IRubiconMarket::makerFeeCall {}),
            batch::view(at, market, dust_a.clone()),
            batch::view(at, market, dust_b.clone()),
        ],
    )
    .await?;
    let matched = batch::decode(&matching, IRubiconMarket::matchingEnabledCall {})?;
    let fees = [
        batch::decode(&trader, IRubiconMarket::getFeeBPSCall {})?,
        batch::decode(&maker, IRubiconMarket::makerFeeCall {})?,
    ];
    let dust = if matched {
        [batch::decode(&da, dust_a)?, batch::decode(&db, dust_b)?]
    } else {
        [U256::ZERO; 2]
    };
    let available = !batch::decode(&closed, IRubiconMarket::isClosedCall {})?
        && (!matched || batch::decode(&enabled, IRubiconMarket::buyEnabledCall {})?);
    let tokens = [a, b];
    let offers = if available {
        let (left, right) = tokio::try_join!(
            side(ctx, market, tokens, 0, at),
            side(ctx, market, tokens, 1, at)
        )?;
        [left, right]
    } else {
        [Vec::new(), Vec::new()]
    };
    if epoch != ctx.settings.generation() {
        return Err(bad(ctx));
    }
    Ok(Book {
        market,
        tokens,
        offers,
        fees: [
            fees[0].try_into().map_err(|_| bad(ctx))?,
            fees[1].try_into().map_err(|_| bad(ctx))?,
        ],
        dust,
    })
}

pub(super) async fn verify(ctx: &Ctx, c: &Candidate) -> Verdict {
    let check = async {
        let address = c.pool.parse::<Address>().map_err(|_| bad(ctx))?;
        let (token0, token1) = super::discover::sorted(c.coin, c.quote);
        let at = super::nami::block(ctx, "latest").await?;
        let book = load(ctx, c.family, address, (token0, token1), &at).await?;
        if book.offers.iter().all(Vec::is_empty) {
            return Err(ctx.err(
                ErrorCode::NotApplicable,
                "Rubicon Classic: у пары нет открытых заявок",
            ));
        }
        Ok(PoolRef::Multi {
            contract: address,
            token0,
            token1,
        })
    }
    .await;
    match check {
        Ok(p) => Verdict::Taken(Box::new(super::verify_woo::found(c, p))),
        Err(e) if e.code == ErrorCode::NotApplicable => Verdict::Rejected(e.message),
        Err(e) => Verdict::Failed(e.message),
    }
}

pub(super) fn topics() -> Vec<alloy_primitives::B256> {
    [
        "emitOffer(bytes32,bytes32,address,address,address,uint128,uint128)",
        "emitCancel(bytes32,bytes32,address,address,address,uint128,uint128)",
        "emitDelete(bytes32,bytes32,address)",
        "emitTake(bytes32,bytes32,address,address,address,address,uint128,uint128)",
    ]
    .map(alloy_primitives::keccak256)
    .to_vec()
}

pub(in crate::venues::bsc_dex) fn request(
    ctx: &Ctx,
    e: &super::registry::Entry,
    token_in: Address,
    amount: U256,
) -> GatewayResult<(Address, alloy_primitives::Bytes)> {
    use crate::core::types::PoolState;
    use alloy::sol_types::SolCall;
    if let PoolState::Rubicon(book) = &e.pool.state {
        use crate::venues::bsc_dex::exec::rubicon_wallet::IBookWallet;
        let quote = book
            .plan(token_in, true, amount)
            .ok_or_else(|| ctx.err(ErrorCode::NoRoute, "Rubicon Classic: нет исполнимых заявок"))?;
        let target = if token_in == book.tokens[0] {
            book.tokens[1]
        } else {
            book.tokens[0]
        };
        let actions = quote
            .actions
            .into_iter()
            .map(|(id, quantity)| IBookWallet::Take { id, quantity })
            .collect();
        return Ok((
            ctx.dex_addr("dex.router", e.family.id)?,
            IBookWallet::quoteCall {
                source: token_in,
                target,
                actions,
                maximum: amount,
            }
            .abi_encode()
            .into(),
        ));
    }
    Err(bad(ctx))
}
pub(in crate::venues::bsc_dex) fn payout(
    e: &super::registry::Entry,
    token_in: Address,
    amount: U256,
    out: &[u8],
) -> Option<U256> {
    use crate::core::types::PoolState;
    use alloy::sol_types::SolCall;
    if let PoolState::Rubicon(book) = &e.pool.state {
        use crate::venues::bsc_dex::exec::rubicon_wallet::IBookWallet;
        let local = book.plan(token_in, true, amount)?;
        let official = IBookWallet::quoteCall::abi_decode_returns_validate(out).ok()?;
        return (local.spent == official.spent && official.nominal == local.received)
            .then_some(official.nominal);
    }
    None
}

//! Состояние DAMM на одном блоке для расчёта следующего обмена, включая комиссии LP.
use super::nami_batch as batch;
use crate::{
    core::types::UmbraeDammState,
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::ctx::Ctx,
};
use alloy_primitives::{Address, U256};
use serde_json::Value;
alloy::sol! {interface IPair {
    function getReserves() view returns(uint112 x,uint112 y);
    function balanceOf(address account) view returns(uint256);
    function feeToken() view returns(address);
    function volatilityAccumulator() view returns(uint16);
    function baseFeeBps() view returns(uint16);
    function maxFeeBps() view returns(uint16);
    function protocolFeePct() view returns(uint16);
    function lastPrice() view returns(uint256);
    function protocolFeesAccumulated() view returns(uint256);
    function lpFeesAccumulated() view returns(uint256);
    function totalSupply() view returns(uint256);
    function lpFeeIndex() view returns(uint256);
}}
pub(super) async fn read(
    ctx: &Ctx,
    pool: Address,
    tokens: [Address; 2],
    at: &Value,
) -> GatewayResult<UmbraeDammState> {
    use IPair::*;
    let balance = balanceOfCall { account: pool };
    let [res, bal0, bal1, ft, v, b, m, p, lp, pf, lf, ls, li] = batch::read(
        ctx,
        [
            batch::view(at, pool, getReservesCall {}),
            batch::view(at, tokens[0], balance.clone()),
            batch::view(at, tokens[1], balance.clone()),
            batch::view(at, pool, feeTokenCall {}),
            batch::view(at, pool, volatilityAccumulatorCall {}),
            batch::view(at, pool, baseFeeBpsCall {}),
            batch::view(at, pool, maxFeeBpsCall {}),
            batch::view(at, pool, protocolFeePctCall {}),
            batch::view(at, pool, lastPriceCall {}),
            batch::view(at, pool, protocolFeesAccumulatedCall {}),
            batch::view(at, pool, lpFeesAccumulatedCall {}),
            batch::view(at, pool, totalSupplyCall {}),
            batch::view(at, pool, lpFeeIndexCall {}),
        ],
    )
    .await?;
    let r = batch::decode(&res, getReservesCall {})?;
    let ft = batch::decode(&ft, feeTokenCall {})?;
    let fee_side = tokens.iter().position(|a| *a == ft).ok_or_else(|| {
        ctx.err(
            ErrorCode::NotApplicable,
            "Umbrae DAMM: актив комиссии не входит в пару",
        )
    })?;
    Ok(UmbraeDammState {
        tokens,
        reserves: [U256::from(r.x), U256::from(r.y)],
        balances: [
            batch::decode(&bal0, balance.clone())?,
            batch::decode(&bal1, balance)?,
        ],
        fee_side,
        base_fee: batch::decode(&b, baseFeeBpsCall {})?,
        max_fee: batch::decode(&m, maxFeeBpsCall {})?,
        protocol_share: batch::decode(&p, protocolFeePctCall {})?,
        volatility: batch::decode(&v, volatilityAccumulatorCall {})?,
        last_price: batch::decode(&lp, lastPriceCall {})?,
        protocol_fees: batch::decode(&pf, protocolFeesAccumulatedCall {})?,
        lp_fees: batch::decode(&lf, lpFeesAccumulatedCall {})?,
        lp_supply: batch::decode(&ls, totalSupplyCall {})?,
        lp_index: batch::decode(&li, lpFeeIndexCall {})?,
    })
}

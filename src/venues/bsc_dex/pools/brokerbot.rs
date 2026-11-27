//! Целые акции и лицензия Brokerbot. Цена, остатки, права двух направлений и роли измеряются вместе.
use super::{
    brokerbot_profile::{invalid, markets},
    families::Family,
    nami_batch as batch,
};
use crate::{
    core::types::BrokerbotState, gateway::errors::GatewayResult, venues::bsc_dex::ctx::Ctx,
};
use alloy_primitives::{Address, U256, keccak256};
use serde_json::{Value, json};
alloy::sol! { interface IBroker {
    function getBrokerbot(address cash,address share) external view returns(address);
    function getBuyPrice(uint256 shares) external view returns(uint256);
    function getSellPrice(uint256 shares) external view returns(uint256);
    function getShares(uint256 amount) external view returns(uint256);
    function getLicenseFee(uint256 amount) external view returns(uint256);
    function getPrice() external view returns(uint256);
    function increment() external view returns(uint256);
    function driftStart() external view returns(uint256);
    function getPriceAtTime(uint256 timestamp) external view returns(uint256);
    function hasDrift() external view returns(bool);
    function settings() external view returns(uint256);
    function base() external view returns(address);
    function token() external view returns(address);
    function paymenthub() external view returns(address);
    function owner() external view returns(address);
    function decimals() external view returns(uint8);
    function balanceOf(address who) external view returns(uint256);
    function totalSupply() external view returns(uint256);
}}
pub(crate) async fn market(
    ctx: &Ctx,
    f: &Family,
    broker: Address,
    edge: (Address, Address),
    at: &Value,
) -> GatewayResult<BrokerbotState> {
    let gen_at = ctx.settings.generation();
    let p = markets(ctx, f)?
        .remove(&broker)
        .ok_or_else(|| invalid(ctx))?;
    if ctx.chain_id() != Some(1)
        || at["requireCanonical"] != true
        || at["blockHash"].as_str().is_none()
        || !(edge == (p.cash, p.share) || edge == (p.share, p.cash))
    {
        return Err(invalid(ctx));
    }
    let registry = ctx.dex_addr("dex.factory", f.id)?;
    let registry_hash = super::v2_pair_binding::expected(ctx, f.id, "dex.factory_runtime_hash")?;
    let lookup = IBroker::getBrokerbotCall {
        cash: p.cash,
        share: p.share,
    };
    let bal = IBroker::balanceOfCall { who: broker };
    let [
        registry_code,
        broker_code,
        hub_code,
        listed,
        cash,
        share,
        hub,
        decimals,
        flags,
        price,
        step,
        drift,
        b0,
        b1,
        supply,
    ] = batch::read(
        ctx,
        [
            ("eth_getCode", json!([registry, at])),
            ("eth_getCode", json!([broker, at])),
            ("eth_getCode", json!([p.hub, at])),
            batch::view(at, registry, lookup.clone()),
            batch::view(at, broker, IBroker::baseCall {}),
            batch::view(at, broker, IBroker::tokenCall {}),
            batch::view(at, broker, IBroker::paymenthubCall {}),
            batch::view(at, p.share, IBroker::decimalsCall {}),
            batch::view(at, broker, IBroker::settingsCall {}),
            batch::view(at, broker, IBroker::getPriceCall {}),
            batch::view(at, broker, IBroker::incrementCall {}),
            batch::view(at, broker, IBroker::hasDriftCall {}),
            batch::view(at, p.cash, bal.clone()),
            batch::view(at, p.share, bal.clone()),
            batch::view(at, p.share, IBroker::totalSupplyCall {}),
        ],
    )
    .await?;
    let registered = batch::decode(&listed, lookup)? == broker;
    if keccak256(registry_code) != registry_hash
        || keccak256(broker_code) != p.code
        || keccak256(hub_code) != p.hub_hash
        || batch::decode(&cash, IBroker::baseCall {})? != p.cash
        || batch::decode(&share, IBroker::tokenCall {})? != p.share
        || batch::decode(&hub, IBroker::paymenthubCall {})? != p.hub
        || batch::decode(&decimals, IBroker::decimalsCall {})? != 0
    {
        return Err(invalid(ctx));
    }
    if let Some(owner) = p.issuer {
        let [b, t] = batch::read(
            ctx,
            [
                batch::view(at, broker, IBroker::ownerCall {}),
                batch::view(at, p.share, IBroker::ownerCall {}),
            ],
        )
        .await?;
        if p.hub != ctx.dex_addr("dex.quoter", f.id)?
            || [b, t]
                .iter()
                .any(|r| batch::decode(r, IBroker::ownerCall {}).ok() != Some(owner))
        {
            return Err(invalid(ctx));
        }
    } else if !registered {
        return Err(invalid(ctx));
    }
    let flags = batch::decode(&flags, IBroker::settingsCall {})?;
    let drift = batch::decode(&drift, IBroker::hasDriftCall {})?;
    let price = batch::decode(&price, IBroker::getPriceCall {})?;
    let base_price = if drift {
        let [start] =
            batch::read(ctx, [batch::view(at, broker, IBroker::driftStartCall {})]).await?;
        let timestamp = batch::decode(&start, IBroker::driftStartCall {})?;
        let q = IBroker::getPriceAtTimeCall { timestamp };
        let [base] = batch::read(ctx, [batch::view(at, broker, q.clone())]).await?;
        batch::decode(&base, q)?
    } else {
        price
    };
    let state = BrokerbotState {
        broker,
        cash: p.cash,
        share: p.share,
        hub: p.hub,
        registered,
        cash_balance: batch::decode(&b0, bal.clone())?,
        share_balance: batch::decode(&b1, bal)?,
        buying: flags.bit(0),
        selling: flags.bit(1),
        price,
        base_price,
        increment: batch::decode(&step, IBroker::incrementCall {})?,
        drift,
        supply: batch::decode(&supply, IBroker::totalSupplyCall {})?,
        average_first: p.average,
        license_bps: p.license_bps,
    };
    if p.license_bps != 0 {
        let fee = IBroker::getLicenseFeeCall {
            amount: U256::from(10_000),
        };
        let [answer] = batch::read(ctx, [batch::view(at, broker, fee.clone())]).await?;
        if batch::decode(&answer, fee)? != U256::from(p.license_bps) {
            return Err(invalid(ctx));
        }
    }
    if state.share_balance > state.supply || gen_at != ctx.settings.generation() {
        return Err(invalid(ctx));
    }
    Ok(state)
}

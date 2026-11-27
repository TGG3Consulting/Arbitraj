//! Проверка неизменяемого ERC20 моста и обеих сторон расчётного актива.
use super::super::ctx::Ctx;
use super::{nami, nami_batch as batch};
use crate::{
    gateway::errors::{ErrorCode, GatewayResult},
    settings::cycle_cash::Profile,
};
use alloy_primitives::keccak256;
use serde_json::json;

alloy::sol! {interface IBridgedCash {
    function l1Token() external view returns(address);
    function l2Bridge() external view returns(address);
    function decimals() external view returns(uint8);
    function symbol() external view returns(string);
}}
// Standard bridged Ethereum USDT carries the same symbol and precision as its backing asset.
const USDT_SYMBOL: &str = "USDT";
const USDT_DECIMALS: u8 = 6;
pub(in crate::venues::bsc_dex) async fn verify(ctx: &Ctx, p: Profile) -> GatewayResult<u8> {
    let fail = || {
        ctx.err(
            ErrorCode::NoRoute,
            "Не подтверждена исходная сеть, реализация или валюта расчётного USDT",
        )
    };
    if ctx.chain_id() != Some(p.chain) {
        return Err(fail());
    }
    let tag = nami::block(ctx, "finalized").await?;
    let [code, r, b, d, s] = batch::read(
        ctx,
        [
            ("eth_getCode", json!([p.token, tag])),
            batch::view(&tag, p.token, IBridgedCash::l1TokenCall {}),
            batch::view(&tag, p.token, IBridgedCash::l2BridgeCall {}),
            batch::view(&tag, p.token, IBridgedCash::decimalsCall {}),
            batch::view(&tag, p.token, IBridgedCash::symbolCall {}),
        ],
    )
    .await?;
    let decimals = batch::decode(&d, IBridgedCash::decimalsCall {})?;
    if code.is_empty()
        || keccak256(&code) != p.runtime
        || batch::decode(&r, IBridgedCash::l1TokenCall {})? != p.remote
        || batch::decode(&b, IBridgedCash::l2BridgeCall {})? != p.bridge
        || decimals != USDT_DECIMALS
        || batch::decode(&s, IBridgedCash::symbolCall {})? != USDT_SYMBOL
    {
        return Err(fail());
    }
    Ok(decimals)
}

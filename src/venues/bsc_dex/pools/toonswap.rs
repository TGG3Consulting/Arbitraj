//! Cold read of the factory fee split and pair balances on one canonical Ethereum block.
use super::super::{ctx::Ctx, node::abi::IERC20};
use super::{
    abi_v2v3::IV2Pair, families::Family, load::Loaded, nami_batch as batch, v2_pair_binding,
};
use crate::{
    core::types::{PoolState, ToonState},
    gateway::errors::{ErrorCode, GatewayResult},
};
use alloy_primitives::{Address, U256};
use serde_json::Value;
alloy::sol! {
    function REFERRAL_FEE() external view returns(uint256);
    function TREASURY_FEE() external view returns(uint256);
    function dibs() external view returns(address);
    function treasury() external view returns(address);
}
pub(crate) async fn read(
    ctx: &Ctx,
    f: &Family,
    pool: Address,
    tokens: (Address, Address),
    at: &Value,
) -> GatewayResult<Loaded> {
    let rev = ctx.settings.generation();
    let factory = ctx.dex_addr("dex.factory", f.id)?;
    let bal = IERC20::balanceOfCall { owner: pool };
    let rs = IV2Pair::getReservesCall {};
    let calls = [
        batch::view(at, pool, rs.clone()),
        batch::view(at, tokens.0, bal.clone()),
        batch::view(at, tokens.1, bal.clone()),
        batch::view(at, factory, REFERRAL_FEECall {}),
        batch::view(at, factory, TREASURY_FEECall {}),
        batch::view(at, factory, dibsCall {}),
        batch::view(at, factory, treasuryCall {}),
    ];
    let ((), [r, a, b, d, t, x, y]) = tokio::try_join!(
        v2_pair_binding::code(ctx, f.id, pool, at),
        batch::read(ctx, calls)
    )?;
    if rev != ctx.settings.generation() {
        return Err(ctx.err(ErrorCode::StaleData, "ToonSwap: изменился профиль"));
    }
    let reserves = batch::decode(&r, rs)?;
    let state = ToonState {
        pool,
        reserves: [U256::from(reserves.reserve0), U256::from(reserves.reserve1)],
        balances: [batch::decode(&a, bal.clone())?, batch::decode(&b, bal)?],
        rates: [
            batch::decode(&d, REFERRAL_FEECall {})?,
            batch::decode(&t, TREASURY_FEECall {})?,
        ],
        recipients: [
            batch::decode(&x, dibsCall {})?,
            batch::decode(&y, treasuryCall {})?,
        ],
    };
    Ok(Loaded {
        state: PoolState::ToonSwap(Box::new(state)),
        fee_pips: Some(2500),
        ticks: None,
        bins: None,
    })
}

//! Фабрика и физические валюты двух поколений Mooniswap на одном каноническом блоке.
use super::{
    families::Family,
    nami_batch as batch,
    verify::{Candidate, Verdict},
};
use crate::{
    core::types::{PoolRef, PoolState},
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::{
        ctx::Ctx,
        node::{call, rpc},
        pools::registry::Entry,
    },
};
use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256, keccak256};
use serde_json::{Value, json};
alloy::sol! {interface IMoon{
 function getTokens() external view returns(address[]);
 function pools(address a,address b) external view returns(address);
 function isPool(address p) external view returns(bool);
 function factory() external view returns(address);
 function mooniswapFactoryGovernance() external view returns(address);
 function isActive() external view returns(bool);
 function getReturn(address src,address dst,uint256 amount) external view returns(uint256);
 function fee() external view returns(uint256);
 function slippageFee() external view returns(uint256);
 function decayPeriod() external view returns(uint256);
 function virtualBalancesForAddition(address token) external view returns(uint216 balance,uint40 time);
 function virtualBalancesForRemoval(address token) external view returns(uint216 balance,uint40 time);
}}
pub(super) fn bad(ctx: &Ctx) -> crate::gateway::errors::GatewayError {
    ctx.err(
        ErrorCode::NotApplicable,
        "Mooniswap: регистрация, физические валюты, тариф или привязка блока изменились",
    )
}
pub(super) fn successor(ctx: &Ctx, f: &Family) -> GatewayResult<bool> {
    match ctx.dex_text("dex.kind", f.id)?.as_str() {
        "mooniswap_v1" => Ok(false),
        "mooniswap_v2" => Ok(true),
        _ => Err(bad(ctx)),
    }
}
pub(super) async fn read<C: SolCall>(
    ctx: &Ctx,
    at: &Value,
    target: Address,
    c: C,
) -> GatewayResult<C::Return> {
    let raw: Bytes = rpc::call(
        ctx,
        "eth_call",
        json!([{"to":target,"data":Bytes::from(c.abi_encode())},at]),
    )
    .await?;
    call::decode::<C>(&raw)
}
pub(crate) async fn market(
    ctx: &Ctx,
    f: &Family,
    pool: Address,
    at: &Value,
) -> GatewayResult<[Address; 2]> {
    let generation = ctx.settings.generation();
    let factory = ctx.dex_addr("dex.factory", f.id)?;
    if pool.is_zero() || at["requireCanonical"] != true || at["blockHash"].as_str().is_none() {
        return Err(bad(ctx));
    }
    let hash = super::v2_pair_binding::expected(ctx, f.id, "dex.factory_runtime_hash")?;
    let v2 = successor(ctx, f)?;
    let t = IMoon::getTokensCall {};
    let l = IMoon::isPoolCall { p: pool };
    let owner = if v2 {
        batch::view(at, pool, IMoon::mooniswapFactoryGovernanceCall {})
    } else {
        batch::view(at, pool, IMoon::factoryCall {})
    };
    let [code, role, list, tokens] = batch::read(
        ctx,
        [
            ("eth_getCode", json!([factory, at])),
            owner,
            batch::view(at, factory, l.clone()),
            batch::view(at, pool, t.clone()),
        ],
    )
    .await?;
    let physical: [Address; 2] = batch::decode(&tokens, t)?
        .try_into()
        .map_err(|_| bad(ctx))?;
    if keccak256(&code) != hash
        || batch::decode(&role, IMoon::factoryCall {})? != factory
        || !batch::decode(&list, l)?
        || physical[0] >= physical[1]
    {
        return Err(bad(ctx));
    }
    let [a, b] = physical;
    let lookup = IMoon::poolsCall { a, b };
    let reverse = IMoon::poolsCall { a: b, b: a };
    let [ab, ba] = batch::read(
        ctx,
        [
            batch::view(at, factory, lookup.clone()),
            batch::view(at, factory, reverse.clone()),
        ],
    )
    .await?;
    if batch::decode(&ab, lookup)? != pool
        || batch::decode(&ba, reverse)? != pool
        || (v2 && !read(ctx, at, factory, IMoon::isActiveCall {}).await?)
        || generation != ctx.settings.generation()
    {
        return Err(bad(ctx));
    }
    Ok(physical)
}
pub(super) fn normalized(pair: [Address; 2], wrapped: Address) -> Option<[Address; 2]> {
    let tok = pair.map(|a| if a.is_zero() { wrapped } else { a });
    (!wrapped.is_zero() && tok[0] != tok[1]).then_some(tok)
}
pub(crate) fn shape(e: &Entry, wrapped: Address) -> Option<[Address; 2]> {
    let PoolState::Mooniswap(s) = &e.pool.state else {
        return None;
    };
    (normalized(s.physical, wrapped)? == s.tokens
        && super::discover::sorted(s.tokens[0], s.tokens[1]) == (e.pool.token0, e.pool.token1))
        .then_some(s.physical)
}
pub(super) async fn verify(ctx: &Ctx, c: &Candidate) -> Verdict {
    let Ok(pool) = c.pool.parse() else {
        return Verdict::Rejected("Mooniswap: неверный адрес пула".into());
    };
    let result = async {
        let at = super::nami::block(ctx, "latest").await?;
        let raw = market(ctx, c.family, pool, &at).await?;
        let p = normalized(raw, ctx.addr("dex.native_wrapped")?).ok_or_else(|| bad(ctx))?;
        if super::discover::sorted(p[0], p[1]) != super::discover::sorted(c.coin, c.quote) {
            return Err(bad(ctx));
        }
        Ok(super::verify_woo::found(c, PoolRef::Contract(pool)))
    }
    .await;
    match result {
        Ok(f) => Verdict::Taken(Box::new(f)),
        Err(e) if e.code == ErrorCode::NotApplicable => Verdict::Rejected(e.message),
        Err(e) => Verdict::Failed(e.message),
    }
}
pub(crate) use super::mooniswap_state::state;
pub(crate) fn request(
    ctx: &Ctx,
    e: &Entry,
    input: Address,
    amount: U256,
) -> GatewayResult<(Address, Bytes)> {
    let PoolState::Mooniswap(s) = &e.pool.state else {
        return Err(bad(ctx));
    };
    let i = s
        .tokens
        .iter()
        .position(|&x| x == input)
        .ok_or_else(|| bad(ctx))?;
    Ok((
        e.pool.reference.address(),
        IMoon::getReturnCall {
            src: s.physical[i],
            dst: s.physical[1 - i],
            amount,
        }
        .abi_encode()
        .into(),
    ))
}
pub(crate) fn payout(_: &Entry, _: Address, _: U256, out: &[u8]) -> Option<U256> {
    IMoon::getReturnCall::abi_decode_returns_validate(out).ok()
}
pub(crate) async fn out_at(
    ctx: &Ctx,
    e: &Entry,
    input: Address,
    amount: U256,
    at: &Value,
) -> GatewayResult<U256> {
    let (pool, data) = request(ctx, e, input, amount)?;
    let raw: Bytes = rpc::call(ctx, "eth_call", json!([{"to":pool,"data":data},at])).await?;
    payout(e, input, amount, &raw).ok_or_else(|| bad(ctx))
}

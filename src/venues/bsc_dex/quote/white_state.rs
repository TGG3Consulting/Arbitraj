//! Котировка официального WhiteSwap зависит от адреса плательщика. Вызов
//! ArbExecutor входит в обычный swap и всегда использует комиссию 30 б.п.
use super::super::{
    ctx::Ctx,
    node::rpc,
    pools::{families::Family, registry::Entry},
};
use crate::gateway::errors::{ErrorCode, GatewayResult};
use alloy::sol;
use alloy::sol_types::SolCall;
use alloy_primitives::{Address, B256, Bytes, U256};
use serde_json::{Value, json};

sol! { interface IReward { function balanceOf(address owner) external view returns(uint256); } }

#[derive(Clone, Copy, serde::Deserialize)]
pub(crate) struct Head {
    pub hash: B256,
    pub number: alloy_primitives::U64,
    pub timestamp: alloy_primitives::U64,
}
impl Head {
    pub fn selector(self) -> Value {
        json!({"blockHash":self.hash,"requireCanonical":true})
    }
}

pub(crate) async fn head(ctx: &Ctx, tag: &str) -> GatewayResult<Head> {
    rpc::call(ctx, "eth_getBlockByNumber", json!([tag, false])).await
}

pub fn fee(balance: U256, is_contract: bool) -> u32 {
    if is_contract {
        return 3000;
    }
    let whole = balance / U256::from(10u64.pow(18));
    let discount = [100u64, 1_000, 10_000, 100_000, 1_000_000]
        .into_iter()
        .filter(|n| whole >= U256::from(*n))
        .count() as u32;
    3000 - 100 * discount
}

pub(crate) async fn wallet_fee(
    ctx: &Ctx,
    f: &Family,
    payer: Address,
    tag: &Value,
) -> GatewayResult<u32> {
    let wse = ctx.dex_addr("dex.whiteswap_reward_token", f.id)?;
    let data = Bytes::from(IReward::balanceOfCall { owner: payer }.abi_encode());
    let (code, balance) = tokio::try_join!(
        rpc::call::<Bytes>(ctx, "eth_getCode", json!([payer, tag])),
        rpc::call::<Bytes>(ctx, "eth_call", json!([{"to":wse,"data":data},tag]))
    )?;
    if balance.len() != 32 {
        return Err(ctx.err(
            ErrorCode::BadResponse,
            "WhiteSwap: WSE.balanceOf не вернул слово ABI",
        ));
    }
    Ok(fee(U256::from_be_slice(&balance), !code.is_empty()))
}

pub(crate) async fn checked_fee(
    ctx: &Ctx,
    e: &Entry,
    payer: Address,
    at: &Value,
) -> GatewayResult<u32> {
    let ((), fee) = tokio::try_join!(
        super::super::pools::whiteswap::check_router(ctx, e.family, e.pool.reference.address(), at),
        wallet_fee(ctx, e.family, payer, at)
    )?;
    Ok(fee)
}

pub(crate) async fn verified(ctx: &Ctx, e: &Entry, payer: Address, h: Head) -> GatewayResult<u32> {
    let tag = h.selector();
    let (_, pips) = tokio::try_join!(
        super::super::pools::whiteswap::check(
            ctx,
            e.family,
            e.pool.reference.address(),
            (e.pool.token0, e.pool.token1),
            &tag
        ),
        checked_fee(ctx, e, payer, &tag)
    )?;
    Ok(pips)
}

pub(crate) async fn refresh(ctx: &Ctx, e: &mut Entry, tag: &str) -> GatewayResult<()> {
    let payer = super::super::wallet::signer::address(ctx)?;
    let h = head(ctx, tag).await?;
    let at = h.selector();
    let ((reserve0, reserve1), pips) = tokio::try_join!(
        super::super::pools::whiteswap::check(
            ctx,
            e.family,
            e.pool.reference.address(),
            (e.pool.token0, e.pool.token1),
            &at
        ),
        checked_fee(ctx, e, payer, &at)
    )?;
    e.pool.state = crate::core::types::PoolState::V2 { reserve0, reserve1 };
    e.pool.fee_pips = Some(pips);
    e.pool.block = h.number.to();
    Ok(())
}

type WalletKey = (&'static str, Address, Address, u64);
#[derive(Default)]
pub(in crate::venues::bsc_dex) struct Cache(
    std::sync::Mutex<std::collections::HashMap<WalletKey, (u64, Option<u32>)>>,
);

/// Индикативная математика остаётся локальной. При первом наблюдении кошелька
/// фоновое чтение по его блоку прогревает скидку для следующих объёмов.
pub(super) fn preliminary(q: &super::Quoting, f: &'static Family, payer: Address) -> Option<u32> {
    let ctx = &q.ctx;
    let token = ctx.dex_addr("dex.whiteswap_reward_token", f.id).ok()?;
    let block = q
        .frozen_head
        .or_else(|| q.heads.latest().map(|h| h.number))?;
    let key = (f.id, payer, token, ctx.int("dex.evm_chain_id").ok()?);
    let mut cache = ctx
        .evm_chain
        .white()
        .0
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if let Some((n, v)) = cache.get(&key)
        && *n == block
    {
        return Some(v.unwrap_or(3000));
    }
    cache.insert(key, (block, None));
    let ctx = ctx.clone();
    tokio::spawn(async move {
        let pips = async {
            let h = head(&ctx, &format!("{block:#x}")).await?;
            wallet_fee(&ctx, f, payer, &h.selector()).await
        }
        .await
        .ok();
        let mut cache = ctx
            .evm_chain
            .white()
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some((n, value)) = cache.get_mut(&key)
            && *n == block
        {
            *value = pips;
        }
    });
    Some(3000)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn funded_wallet_tiers_and_contracts_match_the_verified_router() {
        let j: Value = serde_json::from_str(include_str!(
            "../../../../tests/data/first_run_dex_2026-11-26/whiteswap_wallets.json"
        ))
        .unwrap();
        for wallet in j["rows"]
            .as_array()
            .unwrap()
            .iter()
            .chain(j["boundaries"].as_array().unwrap())
        {
            let w = wallet["wse"].as_str().unwrap().parse().unwrap();
            assert_eq!(
                fee(w, false),
                3000 - 100 * wallet["tier"].as_u64().unwrap() as u32
            );
            assert_eq!(fee(w, true), 3000);
        }
        let unit = U256::from(10u64.pow(18));
        for (i, limit) in [100, 1000, 10000, 100000, 1000000u64]
            .into_iter()
            .enumerate()
        {
            let threshold = U256::from(limit) * unit;
            assert_eq!(fee(threshold - U256::from(1), false), 3000 - 100 * i as u32);
            assert_eq!(fee(threshold, false), 2900 - 100 * i as u32);
        }
    }
}

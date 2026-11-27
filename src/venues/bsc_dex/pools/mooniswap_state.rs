//! Снимок фактических остатков, времени и тарифов Mooniswap. Все чтения используют хеш принятого блока.
use super::{
    families::Family,
    mooniswap::{self, IMoon, bad},
    nami_batch as batch,
};
use crate::{
    core::types::MooniswapState,
    gateway::errors::GatewayResult,
    venues::bsc_dex::{ctx::Ctx, node::rpc},
};
use alloy_primitives::{Address, B256, U64, U256};
use serde::Deserialize;
use serde_json::{Value, json};
#[derive(Deserialize)]
struct Header {
    hash: B256,
    timestamp: U64,
}
async fn token(
    ctx: &Ctx,
    pool: Address,
    token: Address,
    at: &Value,
) -> GatewayResult<(U256, (U256, u64), (U256, u64))> {
    let a = IMoon::virtualBalancesForAdditionCall { token };
    let r = IMoon::virtualBalancesForRemovalCall { token };
    let virtuals = batch::read(
        ctx,
        [
            batch::view(at, pool, a.clone()),
            batch::view(at, pool, r.clone()),
        ],
    );
    let cash = async {
        if token.is_zero() {
            rpc::call(ctx, "eth_getBalance", json!([pool, at])).await
        } else {
            super::dagrow::balance(ctx, token, pool, at).await
        }
    };
    let (physical, [addition, removal]) = tokio::try_join!(cash, virtuals)?;
    let additions = batch::decode(&addition, a)?;
    let removals = batch::decode(&removal, r)?;
    Ok((
        physical,
        (U256::from(additions.balance), additions.time.to()),
        (U256::from(removals.balance), removals.time.to()),
    ))
}
pub(crate) async fn state(
    ctx: &Ctx,
    f: &Family,
    pool: Address,
    pair: (Address, Address),
    at: &Value,
) -> GatewayResult<MooniswapState> {
    let generation = ctx.settings.generation();
    let physical = mooniswap::market(ctx, f, pool, at).await?;
    let tokens =
        mooniswap::normalized(physical, ctx.addr("dex.native_wrapped")?).ok_or_else(|| bad(ctx))?;
    if super::discover::sorted(tokens[0], tokens[1]) != super::discover::sorted(pair.0, pair.1) {
        return Err(bad(ctx));
    }
    let hash: B256 = serde_json::from_value(at["blockHash"].clone()).map_err(|_| bad(ctx))?;
    let v2 = mooniswap::successor(ctx, f)?;
    let slippage = async {
        if v2 {
            mooniswap::read(ctx, at, pool, IMoon::slippageFeeCall {}).await
        } else {
            Ok(U256::ZERO)
        }
    };
    let (a, b, header, fee, decay, slippage_fee) = tokio::try_join!(
        token(ctx, pool, physical[0], at),
        token(ctx, pool, physical[1], at),
        rpc::call::<Header>(ctx, "eth_getBlockByHash", json!([hash, false])),
        mooniswap::read(ctx, at, pool, IMoon::feeCall {}),
        mooniswap::read(ctx, at, pool, IMoon::decayPeriodCall {}),
        slippage
    )?;
    let decay: u64 = decay.try_into().map_err(|_| bad(ctx))?;
    if header.hash != hash || decay == 0 || generation != ctx.settings.generation() {
        return Err(bad(ctx));
    }
    Ok(MooniswapState {
        tokens,
        physical,
        balances: [a.0, b.0],
        addition: [a.1, b.1],
        removal: [a.2, b.2],
        timestamp: header.timestamp.to(),
        decay,
        fee,
        slippage_fee,
        successor: v2,
    })
}

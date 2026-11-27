//! Ступени канонических котировок Curve Stabull. В обеих сторонах Curve возвращает сырые единицы.
use super::{
    families::Family,
    integral_quotes::levels,
    ladder_counter, load_metric,
    registry::{Entry, Registry},
    stabull::IStabull,
};
use crate::{
    core::types::PoolState,
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::{ctx::Ctx, tokens::Tokens},
};
use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256};
use serde_json::Value;

pub(crate) fn request(
    ctx: &Ctx,
    e: &Entry,
    input: Address,
    amount: U256,
) -> GatewayResult<(Address, Bytes)> {
    let (a, b) = (e.pool.token0, e.pool.token1);
    let output = if input == a {
        b
    } else if input == b {
        a
    } else {
        return Err(ctx.err(ErrorCode::NoRoute, "Stabull: посторонняя валюта входа"));
    };
    Ok((
        e.pool.reference.address(),
        IStabull::viewOriginSwapCall {
            a: input,
            b: output,
            amount,
        }
        .abi_encode()
        .into(),
    ))
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn ladder(
    ctx: &Ctx,
    tokens: &Tokens,
    reg: &Registry,
    f: &Family,
    pool: Address,
    pair: (Address, Address),
    counter: Address,
    at: &Value,
) -> GatewayResult<PoolState> {
    let coin = if counter == pair.0 { pair.1 } else { pair.0 };
    let (buys, mut sells) = tokio::try_join!(
        ladder_counter::amounts(ctx, tokens, reg, f.id, counter),
        ladder_counter::amounts(ctx, tokens, reg, f.id, coin)
    )?;
    let quote = |a, b, amount| IStabull::viewOriginSwapCall { a, b, amount };
    let bought = levels(ctx, pool, (counter, coin), &buys, at, quote).await?;
    sells.extend(bought.iter().map(|point| point.amount_out));
    sells.sort_unstable();
    sells.dedup();
    let sold = levels(ctx, pool, (coin, counter), &sells, at, quote).await?;
    Ok(load_metric::state(
        pair,
        coin,
        pair.0,
        (!bought.is_empty() || !sold.is_empty()).then_some(0),
        bought,
        sold,
    ))
}

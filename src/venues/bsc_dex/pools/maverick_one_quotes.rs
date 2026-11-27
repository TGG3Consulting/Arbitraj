//! Ступени официального PoolInformation. Финальный вызов модуля требует весь вход и сверяет получателя.
use super::{
    families::Family, ladder_counter, load_metric, maverick_one::quote, nami_batch as batch,
    registry::Registry,
};
use crate::{
    core::types::{LadderPoint, PoolState},
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::{ctx::Ctx, node::rpc, tokens::Tokens},
};
use alloy_primitives::{Address, Bytes, U256};
use serde_json::Value;
async fn pass(
    ctx: &Ctx,
    viewer: Address,
    pair: Address,
    forward: bool,
    sizes: &[U256],
    at: &Value,
) -> GatewayResult<Vec<LadderPoint>> {
    let orders: Vec<_> = sizes
        .iter()
        .filter_map(|&n| Some((n, quote(pair, forward, n)?)))
        .collect();
    if orders.is_empty() {
        return Ok(Vec::new());
    }
    let calls: Vec<_> = orders
        .iter()
        .map(|(_, q)| batch::view(at, viewer, q.clone()))
        .collect();
    let mut points = Vec::new();
    for ((amount_in, order), reply) in orders.into_iter().zip(rpc::batch(ctx, &calls).await?) {
        let raw = match reply {
            Ok(v) => v,
            Err(e) if e.code == ErrorCode::SwapReverted => continue,
            Err(e) => return Err(e),
        };
        let bytes: Bytes = serde_json::from_str(raw.get())
            .map_err(|err| ctx.err(ErrorCode::BadResponse, err.to_string()))?;
        let amount_out = batch::decode(&bytes, order)?;
        if !amount_out.is_zero() {
            points.push(LadderPoint {
                amount_in,
                amount_out,
            });
        }
    }
    Ok(points)
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn ladder(
    ctx: &Ctx,
    tokens: &Tokens,
    reg: &Registry,
    f: &Family,
    pool: Address,
    pair: [Address; 2],
    counter: Address,
    at: &Value,
) -> GatewayResult<PoolState> {
    let coin = if pair[0] == counter { pair[1] } else { pair[0] };
    let viewer = ctx.dex_addr("dex.quoter", f.id)?;
    let (buys, mut sells) = tokio::try_join!(
        ladder_counter::amounts(ctx, tokens, reg, f.id, counter),
        ladder_counter::amounts(ctx, tokens, reg, f.id, coin)
    )?;
    let bought = pass(ctx, viewer, pool, counter == pair[0], &buys, at).await?;
    sells.extend(bought.iter().map(|p| p.amount_out));
    sells.sort_unstable();
    sells.dedup();
    let sold = pass(ctx, viewer, pool, coin == pair[0], &sells, at).await?;
    Ok(load_metric::state(
        (pair[0], pair[1]),
        coin,
        pair[0],
        (!bought.is_empty() || !sold.is_empty()).then_some(0),
        bought,
        sold,
    ))
}

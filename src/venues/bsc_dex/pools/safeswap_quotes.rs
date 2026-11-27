//! Лесенка чистого WETH бюджета для шага V5. Точный объём перепроверяется на том же блоке.
use super::{
    families::Family,
    ladder_counter, load_metric, nami_batch as batch,
    registry::{Entry, Registry},
    safeswap_trade::{self, ISafeDeployment as Market},
};
use crate::{
    core::types::{LadderPoint, PoolState},
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::{ctx::Ctx, node::rpc, tokens::Tokens},
};
use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256};
use serde_json::Value;

pub(in crate::venues::bsc_dex) fn request(
    ctx: &Ctx,
    e: &Entry,
    input: Address,
    n: U256,
) -> GatewayResult<(Address, Bytes)> {
    let (a, b) = (e.pool.token0, e.pool.token1);
    let output = if input == a {
        b
    } else if input == b {
        a
    } else {
        return Err(ctx.err(ErrorCode::NoRoute, "SafeSwap: вход отсутствует в паре"));
    };
    Ok((
        ctx.dex_addr("dex.router", e.family.id)?,
        Market::quoteCall {
            input,
            output,
            amount: n,
            separate: false,
        }
        .abi_encode()
        .into(),
    ))
}
pub(in crate::venues::bsc_dex) fn payout(raw: &[u8]) -> Option<U256> {
    let row = Market::quoteCall::abi_decode_returns_validate(raw).ok()?;
    (raw.len() == 64 && !row.net.is_zero()).then_some(row.net)
}
async fn pass(
    ctx: &Ctx,
    market: (&Family, Address, Address),
    (input, output): (Address, Address),
    sizes: &[U256],
    at: &Value,
) -> GatewayResult<Vec<LadderPoint>> {
    if sizes.is_empty() {
        return Ok(Vec::new());
    }
    let (f,pool,module)=market;
    let degen=f.variant==super::family_variant::Variant::DegenSwap;
    let calls=sizes.iter().map(|&amount|{
        if degen {
            let (to,data)=super::degenswap_quotes::wire(ctx,f,pool,input,output,amount)?;
            Ok(("eth_call",serde_json::json!([{"to":to,"data":data},at])))
        } else {
            Ok(batch::view(at,module,Market::quoteCall{input,output,amount,separate:false}))
        }
    }).collect::<GatewayResult<Vec<_>>>()?;
    let replies = rpc::batch(ctx, &calls).await?;
    if replies.len() != sizes.len() {
        return Err(ctx.err(
            ErrorCode::BadResponse,
            "SafeSwap: неполный ответ по уровням",
        ));
    }
    let mut points = Vec::new();
    for (amount, r) in sizes.iter().copied().zip(replies) {
        let raw = match r {
            Ok(row) => row,
            Err(e) if e.code == ErrorCode::SwapReverted => continue,
            Err(e) => return Err(e),
        };
        let bytes: Bytes = serde_json::from_str(raw.get())
            .map_err(|e| ctx.err(ErrorCode::BadResponse, e.to_string()))?;
        let amount_out=if degen { super::degenswap_quotes::payout(&bytes) } else { payout(&bytes) };
        if let Some(amount_out) = amount_out {
            points.push(LadderPoint {
                amount_in: amount,
                amount_out,
            });
        }
    }
    Ok(points)
}
#[allow(clippy::too_many_arguments)]
pub(super) async fn load(
    ctx: &Ctx,
    tokens: &Tokens,
    reg: &Registry,
    f: &Family,
    pool: Address,
    pair: (Address, Address),
    counter: Address,
    at: &Value,
) -> GatewayResult<PoolState> {
    let module = Box::pin(safeswap_trade::market(ctx, f, pool, pair, at)).await?;
    let coin = if pair.0 == counter { pair.1 } else { pair.0 };
    let (buy_sizes, mut sell_sizes) = tokio::try_join!(
        ladder_counter::amounts(ctx, tokens, reg, f.id, counter),
        ladder_counter::amounts(ctx, tokens, reg, f.id, coin)
    )?;
    let bought = pass(ctx, (f,pool,module), (counter, coin), &buy_sizes, at).await?;
    sell_sizes.extend(bought.iter().map(|p| p.amount_out));
    sell_sizes.sort_unstable();
    sell_sizes.dedup();
    let sold = pass(ctx, (f,pool,module), (coin, counter), &sell_sizes, at).await?;
    Ok(load_metric::state(
        pair,
        coin,
        pair.0,
        (!bought.is_empty() || !sold.is_empty()).then_some(0),
        bought,
        sold,
    ))
}

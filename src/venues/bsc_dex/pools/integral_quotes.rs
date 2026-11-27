//! Публичные мгновенные quoteSell/quoteBuy. Собственная заявка Relayer в Delay не является второй выплатой.
use super::{
    families::Family,
    integral_size::{self, ISize},
    ladder_counter, load_metric, nami_batch as batch,
    registry::{Entry, Registry},
};
use crate::{
    core::types::{LadderPoint, PoolState},
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::{ctx::Ctx, node::rpc, tokens::Tokens},
};
use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256};
use serde_json::Value;
pub(crate) fn request(
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
        return Err(integral_size::bad(ctx));
    };
    Ok((
        ctx.dex_addr("dex.quoter", e.family.id)?,
        ISize::quoteSellCall {
            a: input,
            b: output,
            amount: n,
        }
        .abi_encode()
        .into(),
    ))
}
pub(crate) async fn required(
    ctx: &Ctx,
    relay: Address,
    a: Address,
    b: Address,
    amount: U256,
    at: &Value,
) -> GatewayResult<U256> {
    let q = ISize::quoteBuyCall { a, b, amount };
    let [raw] = batch::read(ctx, [batch::view(at, relay, q.clone())]).await?;
    let need = batch::decode(&raw, q)?;
    if need.is_zero() {
        return Err(integral_size::bad(ctx));
    }
    Ok(need)
}
pub(crate) fn payout(raw: &[u8]) -> Option<U256> {
    let n = ISize::quoteSellCall::abi_decode_returns_validate(raw).ok()?;
    (raw.len() == 32 && !n.is_zero()).then_some(n)
}
async fn levels(
    ctx: &Ctx,
    relay: Address,
    pair: (Address, Address),
    sizes: &[U256],
    at: &Value,
) -> GatewayResult<Vec<LadderPoint>> {
    if sizes.is_empty() {
        return Ok(Vec::new());
    }
    let calls = sizes
        .iter()
        .map(|&amount| {
            batch::view(
                at,
                relay,
                ISize::quoteSellCall {
                    a: pair.0,
                    b: pair.1,
                    amount,
                },
            )
        })
        .collect::<Vec<_>>();
    let rows = rpc::batch(ctx, &calls).await?;
    if rows.len() != sizes.len() {
        return Err(integral_size::bad(ctx));
    }
    let mut steps = Vec::new();
    for (&amount_in, row) in sizes.iter().zip(rows) {
        let raw = match row {
            Ok(x) => x,
            Err(e) if e.code == ErrorCode::SwapReverted => continue,
            Err(e) => return Err(e),
        };
        let raw: Bytes = serde_json::from_str(raw.get()).map_err(|_| integral_size::bad(ctx))?;
        if raw.len() != 32 {
            return Err(integral_size::bad(ctx));
        }
        if let Some(amount_out) = payout(&raw) {
            steps.push(LadderPoint {
                amount_in,
                amount_out,
            })
        }
    }
    Ok(steps)
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
    let coin = if counter == pair.0 { pair.1 } else { pair.0 };
    // A negative flag publishes no price. Revalidate every role before admitting a positive result.
    let relay = ctx.dex_addr("dex.quoter", f.id)?;
    let flag = ISize::isPairEnabledCall { pair: pool };
    let [raw] = batch::read(ctx, [batch::view(at, relay, flag.clone())]).await?;
    if !batch::decode(&raw, flag)? {
        return Ok(load_metric::state(pair, coin, pair.0, None, vec![], vec![]));
    }
    let (r, enabled) = integral_size::state(ctx, f, pool, pair, at).await?;
    if !enabled || !integral_size::funded(ctx, r, at).await? {
        return Ok(load_metric::state(pair, coin, pair.0, None, vec![], vec![]));
    }
    let (buys, mut sells) = tokio::try_join!(
        ladder_counter::amounts(ctx, tokens, reg, f.id, counter),
        ladder_counter::amounts(ctx, tokens, reg, f.id, coin)
    )?;
    let bought = levels(ctx, r.relay, (counter, coin), &buys, at).await?;
    sells.extend(bought.iter().map(|p| p.amount_out));
    sells.sort_unstable();
    sells.dedup();
    let sold = levels(ctx, r.relay, (coin, counter), &sells, at).await?;
    Ok(load_metric::state(
        pair,
        coin,
        pair.0,
        (!bought.is_empty() || !sold.is_empty()).then_some(0),
        bought,
        sold,
    ))
}

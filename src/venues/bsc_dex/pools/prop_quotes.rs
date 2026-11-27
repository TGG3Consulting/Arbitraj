//! Раздельные публичные котировки выбранной площадки и резервного Uniswap на одном блоке.
//! Для одной суммы и одной продажи доступен один результат публичного поручения.
use super::{
    families::Family,
    ladder_counter, load_metric,
    prop_venue::{self, Roles},
    registry::{Entry, Registry},
};
use crate::{
    core::types::{LadderPoint, PoolState},
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::{ctx::Ctx, exec::prop_abi::IProp, node::rpc, tokens::Tokens, wallet},
};
use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256};
use serde_json::{Value, json};
fn data(venue: Address, i: Address, o: Address, n: U256) -> Bytes {
    IProp::quoteVenueV1Call {
        venue,
        tokenIn: i,
        tokenOut: o,
        amountIn: n,
    }
    .abi_encode()
    .into()
}
fn point(raw: &[u8]) -> Option<(U256, Address)> {
    let v = IProp::quoteVenueV1Call::abi_decode_returns_validate(raw).ok()?;
    (raw.len() == 64 && !v.amountOut.is_zero() && !v.quotedVenue.is_zero())
        .then_some((v.amountOut, v.quotedVenue))
}
pub(crate) fn request(
    ctx: &Ctx,
    e: &Entry,
    i: Address,
    n: U256,
) -> GatewayResult<(Address, Bytes)> {
    let (a, b) = (e.pool.token0, e.pool.token1);
    let o = if i == a {
        b
    } else if i == b {
        a
    } else {
        return Err(prop_venue::bad(ctx));
    };
    Ok((
        ctx.dex_addr("dex.quoter", e.family.id)?,
        data(e.pool.reference.address(), i, o, n),
    ))
}
pub(crate) fn payout(raw: &[u8]) -> Option<U256> {
    Some(point(raw)?.0)
}
async fn levels(
    ctx: &Ctx,
    r: &Roles,
    pair: (Address, Address),
    sizes: &[U256],
    payer: Address,
    at: &Value,
) -> GatewayResult<Vec<LadderPoint>> {
    if sizes.is_empty() {
        return Ok(Vec::new());
    }
    let sources = [r.quoted_venue(), r.fallback()];
    let calls = sizes
        .iter()
        .flat_map(|n| {
            sources.map(|s| {
                (
                    "eth_call",
                    json!([{"from":payer,"to":r.router(),"data":data(s,pair.0,pair.1,*n)},at]),
                )
            })
        })
        .collect::<Vec<_>>();
    let rows = rpc::batch(ctx, &calls).await?;
    if rows.len() != calls.len() {
        return Err(prop_venue::bad(ctx));
    }
    let mut out = Vec::new();
    let mut rows = rows.into_iter();
    for &amount_in in sizes {
        let mut preferred = None;
        for source in sources {
            let raw = match rows.next().ok_or_else(|| prop_venue::bad(ctx))? {
                Ok(x) => x,
                Err(e) if e.code == ErrorCode::SwapReverted => continue,
                Err(e) => return Err(e),
            };
            let v: Bytes = serde_json::from_str(raw.get()).map_err(|_| prop_venue::bad(ctx))?;
            if let Some((p, venue)) = point(&v)
                && venue == source
                && preferred.is_none()
            {
                preferred = Some(p)
            }
        }
        if let Some(amount_out) = preferred {
            out.push(LadderPoint {
                amount_in,
                amount_out,
            })
        }
    }
    Ok(out)
}
#[allow(clippy::too_many_arguments)]
pub(crate) async fn load(
    ctx: &Ctx,
    tokens: &Tokens,
    reg: &Registry,
    f: &Family,
    market: Address,
    pair: (Address, Address),
    cash: Address,
    at: &Value,
) -> GatewayResult<PoolState> {
    let (r, on) = prop_venue::state(ctx, f, market, pair, at).await?;
    let coin = if pair.0 == cash { pair.1 } else { pair.0 };
    if !on {
        return Ok(load_metric::state(pair, coin, pair.0, None, vec![], vec![]));
    }
    let who = wallet::signer::address(ctx)?;
    let (buys, mut sells) = tokio::try_join!(
        ladder_counter::amounts(ctx, tokens, reg, f.id, cash),
        ladder_counter::amounts(ctx, tokens, reg, f.id, coin)
    )?;
    let b = levels(ctx, &r, (cash, coin), &buys, who, at).await?;
    sells.extend(b.iter().map(|p| p.amount_out));
    sells.sort_unstable();
    sells.dedup();
    let s = levels(ctx, &r, (coin, cash), &sells, who, at).await?;
    Ok(load_metric::state(
        pair,
        coin,
        pair.0,
        (!s.is_empty() || !b.is_empty()).then_some(0),
        b,
        s,
    ))
}

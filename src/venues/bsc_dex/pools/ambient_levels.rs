use super::{
    ambient::{self, bad},
    ambient_directory,
    balancer_v1::individual,
    families::Family,
    ladder_counter,
    registry::Registry,
};
use crate::{
    core::types::{
        LadderPoint, PoolRef, PoolState,
        ambient::{Market, State},
    },
    gateway::errors::GatewayResult,
    venues::bsc_dex::{ctx::Ctx, tokens::Tokens},
};
use alloy_primitives::{Address, U256};
use serde_json::Value;
async fn levels(
    ctx: &Ctx,
    impact: Address,
    m: Market,
    buy: bool,
    sizes: &[U256],
    at: &Value,
) -> GatewayResult<(Vec<LadderPoint>, Option<U256>)> {
    let requests: Vec<_> = sizes
        .iter()
        .filter_map(|&q| Some((q, ambient::source(impact, m, buy, true, q)?)))
        .collect();
    let calls: Vec<_> = requests.iter().map(|(_, v)| v.clone()).collect();
    let data = individual(ctx, &calls, at).await?;
    if data.len() != requests.len() {
        return Err(bad(ctx));
    }
    let mut points = Vec::new();
    let mut partial = Vec::new();
    for ((asked, _), row) in requests.into_iter().zip(data) {
        if let Ok(raw) = row
            && let Some(p) = ambient::decode(&raw, buy)
            && p.amount_in <= asked
        {
            if p.amount_in < asked {
                partial.push((asked, p.amount_in, p.amount_out));
            }
            points.push(p)
        }
    }
    points.sort_unstable_by_key(|p| p.amount_in);
    if points
        .windows(2)
        .any(|w| w[0].amount_in == w[1].amount_in && w[0].amount_out != w[1].amount_out)
    {
        return Err(bad(ctx));
    }
    points.dedup_by_key(|p| p.amount_in);
    let cap = points.last().and_then(|last| {
        partial
            .into_iter()
            .filter(|(_, spent, net)| (*spent, *net) == (last.amount_in, last.amount_out))
            .map(|(asked, _, _)| asked)
            .max()
    });
    Ok((points, cap))
}
#[allow(clippy::too_many_arguments)]
pub(super) async fn load(
    ctx: &Ctx,
    t: &Tokens,
    reg: &Registry,
    f: &Family,
    reference: &PoolRef,
    pair: (Address, Address),
    counter: Address,
    head: crate::venues::bsc_dex::quote::white_state::Head,
) -> GatewayResult<PoolState> {
    let PoolRef::Singleton { manager, id } = reference else {
        return Err(bad(ctx));
    };
    let stamp = reg.ambient_stamp(head.number.to(), head.hash);
    let at = &head.selector();
    let r = ambient::bind(ctx, f, at).await?;
    let wrapped = ctx.addr("dex.native_wrapped")?;
    if r.dex != *manager {
        return Err(bad(ctx));
    }
    let m = ambient_directory::resolve(ctx, f, r.dex, *id, pair)?;
    ambient::market(ctx, r, m, at).await?;
    let coins = m.tokens(wrapped).ok_or_else(|| bad(ctx))?;
    let pos = coins
        .iter()
        .position(|a| *a == counter)
        .ok_or_else(|| bad(ctx))?;
    let (buys, mut sells) = tokio::try_join!(
        ladder_counter::amounts(ctx, t, reg, f.id, counter),
        ladder_counter::amounts(ctx, t, reg, f.id, coins[1 - pos])
    )?;
    let buy = levels(ctx, r.impact, m, pos == 0, &buys, at).await?;
    sells.extend(buy.0.iter().map(|p| p.amount_out));
    sells.sort_unstable();
    sells.dedup();
    let sell = levels(ctx, r.impact, m, pos != 0, &sells, at).await?;
    let [(a, pa), (b, pb)] = if pos == 0 { [buy, sell] } else { [sell, buy] };
    Ok(PoolState::Ambient(Box::new(State {
        stamp,
        module: ctx
            .dex_addr("dex.router", f.id)
            .ok()
            .filter(|a| !a.is_zero()),
        dex: r.dex,
        market: m,
        wrapped,
        impact: r.impact,
        levels: [a, b],
        partial_cap: [pa, pb],
    })))
}

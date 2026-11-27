//! The core transfers only the two terminal assets. Intermediate fills are the ordered original LogSwap events.
use super::fluid_hop_sizing::paid;
use super::{
    Quoting, costs, graph_legs,
    path::Path,
    pool_tax,
    route_calc::{self, Leg},
    routing::Input,
    select,
    steps::Names,
    v2_execution::OptionQuote,
    via::Via,
    white_state,
};
use crate::{
    core::types::{RouteHop, RouteMode, RouterFeeMode, Side},
    gateway::errors::GatewayResult,
    venues::bsc_dex::{
        gas_ticker,
        pools::{fluid_dex_lite as lite, registry::Entry},
    },
};
use alloy_primitives::{Address, U256};
use rust_decimal::Decimal;
use std::{collections::VecDeque, sync::Arc};

pub(crate) fn selected(hops: &[RouteHop]) -> Option<(Address, U256)> {
    let mode = hops.first()?.fee_mode?;
    let (target, principal) = match mode {
        RouterFeeMode::FluidHop { target, principal }
        | RouterFeeMode::MotoswapPath { target, principal } => (target, principal),
        _ => return None,
    };
    (hops.len() >= 2
        && !target.is_zero()
        && !principal.is_zero()
        && hops.iter().all(|h| h.fee_mode == Some(mode)))
    .then_some((target, principal))
}
pub(super) fn sides(v: &Via<'_>) -> (Address, Address) {
    if v.side == Side::Buy {
        (v.usdt, v.coin)
    } else {
        (v.coin, v.usdt)
    }
}

fn routes(v: &Via<'_>, i: &Input) -> Vec<Vec<Arc<Entry>>> {
    let edges = v.q.reg.multi_market_edges();
    if edges.is_empty() {
        return vec![];
    }
    let (start, end) = sides(v);
    if start == end {
        return vec![];
    }
    let ctx = &v.q.ctx;
    let limit = super::bound_set::hop_limit(ctx).unwrap_or(super::via::CLASSIC_MAX_HOPS);
    let configured = ctx
        .int("dex.route_search_max_paths")
        .ok()
        .and_then(|n| usize::try_from(n).ok())
        .filter(|n| *n > 0);
    // Even with a cycle there are at most this many queued prefixes in one selection.
    let limit_paths = configured.unwrap_or_else(|| edges.len().saturating_mul(limit).max(1));
    let mut queue = VecDeque::from([(start, Vec::<Arc<Entry>>::new())]);
    let mut results = Vec::new();
    let mut expanded = 0;
    while let Some((asset, prefix)) = queue.pop_front() {
        if expanded >= limit_paths.saturating_mul(limit) {
            break;
        }
        expanded += 1;
        for entry in &edges {
            if prefix
                .first()
                .is_some_and(|e| e.family.id != entry.family.id)
            {
                continue;
            }
            let next = if let Some(s) = lite::shape(entry) {
                let Some(side) = s.side(asset) else { continue };
                if s.hop_module.is_none()
                    || s.hop_module != ctx.dex_addr("dex.router", entry.family.id).ok()
                    || s.levels[side].is_empty()
                {
                    continue;
                }
                let Some(tokens) = s.market.tokens(s.wrapped) else {
                    continue;
                };
                tokens[1 - side]
            } else {
                if prefix
                    .iter()
                    .any(|e| e.pool.reference == entry.pool.reference)
                {
                    continue;
                }
                if asset == entry.pool.token0 {
                    entry.pool.token1
                } else if asset == entry.pool.token1 {
                    entry.pool.token0
                } else {
                    continue;
                }
            };
            let depth = prefix.len() + 1;
            let terminal = next == end;
            if !terminal
                && (depth >= limit
                    || !i
                        .mids
                        .iter()
                        .any(|(m, _)| m.address == next && v.mid_ok(m).is_ok()))
            {
                continue;
            }
            if !v.q.exec_quote
                && prefix
                    .iter()
                    .any(|e| e.pool.reference == entry.pool.reference)
            {
                continue;
            }
            let mut path = prefix.clone();
            path.push(entry.clone());
            if terminal {
                if depth >= 2 {
                    results.push(path);
                    if results.len() >= limit_paths {
                        return results;
                    }
                }
            } else if queue.len() < limit_paths.saturating_mul(limit) {
                queue.push_back((next, path));
            }
        }
    }
    results
}

async fn legs(v: &Via<'_>, entries: &[Arc<Entry>]) -> GatewayResult<Vec<Leg>> {
    let ctx = &v.q.ctx;
    let (mut input, end) = sides(v);
    let mut legs = Vec::with_capacity(entries.len());
    let mids = v.q.mids.active(ctx);
    for (n, e) in entries.iter().enumerate() {
        if !e.valid || e.asleep {
            return Err(lite::bad(ctx));
        }
        let output = if let Some(s) = lite::shape(e) {
            if !s.stamp.current() {
                return Err(lite::bad(ctx));
            }
            let side = s.side(input).ok_or_else(|| lite::bad(ctx))?;
            s.market.tokens(s.wrapped).ok_or_else(|| lite::bad(ctx))?[1 - side]
        } else if super::super::exec::motoswap_plan::shape(e).is_some() && input == e.pool.token0 {
            e.pool.token1
        } else if super::super::exec::motoswap_plan::shape(e).is_some() && input == e.pool.token1 {
            e.pool.token0
        } else {
            return Err(lite::bad(ctx));
        };
        if n + 1 < entries.len() {
            let m = mids
                .iter()
                .find(|m| m.address == output)
                .ok_or_else(|| lite::bad(ctx))?;
            v.mid_ok(m).map_err(|_| lite::bad(ctx))?;
        }
        let hs = graph_legs::hop_side(e, input).ok_or_else(|| lite::bad(ctx))?;
        let u = select::usable(v.q, e, hs, v.usdt_dec).map_err(|_| lite::bad(ctx))?;
        pool_tax::v2_only(e, &u).map_err(|_| lite::bad(ctx))?;
        let (din, dout) = tokio::try_join!(
            v.q.tokens.decimals(ctx, input),
            v.q.tokens.decimals(ctx, output)
        )?;
        legs.push(Leg {
            entry: (**e).clone(),
            usable: u,
            token_in: input,
            token_out: output,
            dec_in: din,
            dec_out: dout,
        });
        input = output;
    }
    if input != end {
        return Err(lite::bad(ctx));
    }
    if entries
        .first()
        .is_some_and(|e| super::super::exec::motoswap_plan::shape(e).is_some())
    {
        super::motoswap_path::fees(&mut legs).ok_or_else(|| lite::bad(ctx))?;
    }
    Ok(legs)
}

pub(in crate::venues::bsc_dex) async fn reprice(
    v: &Via<'_>,
    hops: &[RouteHop],
    qty: Decimal,
) -> GatewayResult<Path> {
    let ctx = &v.q.ctx;
    let (target, principal) = selected(hops).ok_or_else(|| lite::bad(ctx))?;
    let entries = hops
        .iter()
        .map(|h| {
            v.q.reg
                .entry(&h.pool)
                .filter(|e| e.family.id == h.family && e.pool.kind == h.kind)
                .ok_or_else(|| lite::bad(ctx))
        })
        .collect::<GatewayResult<Vec<_>>>()?;
    let legs = legs(v, &entries).await?;
    let h = white_state::head(ctx, "latest").await?;
    Ok(paid(v, &legs, qty, h, (target, Some(principal))).await?.0)
}

pub(in crate::venues::bsc_dex) async fn candidates(q: &Quoting, i: &Input, native: bool) -> Vec<OptionQuote> {
    if native {
        return vec![];
    }
    let v = Via {
        q,
        coin: i.coin,
        usdt: i.usdt,
        side: i.side,
        taxes: &i.taxes,
        coin_dec: i.coin_dec,
        usdt_dec: i.usdt_dec,
    };
    let paths = routes(&v, i);
    if paths.is_empty() {
        return vec![];
    }
    let h = if q.exec_quote {
        let Ok(h) = white_state::head(&q.ctx, "latest").await else {
            return vec![];
        };
        Some(h)
    } else {
        None
    };
    let Ok(gas) = gas_ticker::read(&q.ctx) else {
        return vec![];
    };
    let names = Names {
        coin: (i.coin, &i.asset.ticker),
        usdt: (i.usdt, &i.usdt_ticker),
        mids: &q.mids,
        gas: &gas,
    };
    let mut out = Vec::new();
    for entries in paths {
        let Ok(legs) = legs(&v, &entries).await else {
            continue;
        };
        let first = &entries[0];
        let moto = super::super::exec::motoswap_plan::shape(first).is_some();
        let mut targets = q
            .ctx
            .dex_addr("dex.router", first.family.id)
            .ok()
            .filter(|a| !a.is_zero())
            .into_iter()
            .collect::<Vec<_>>();
        if let crate::core::types::PoolState::Motoswap(s) = &first.pool.state {
            targets.retain(|a| Some(*a) == s.path_module);
        }
        if moto
            && matches!(&first.pool.state,crate::core::types::PoolState::Motoswap(s) if s.fee_router_frozen)
            && crate::venues::bsc_dex::pools::exact_wallet_tokens::eligible_input(
                &q.ctx,
                first.family.id,
                sides(&v).0,
            )
            && let Ok(fee) = q.ctx.dex_addr("dex.quoter", first.family.id)
        {
            targets.push(fee);
        }
        for target in targets {
            let (path, approval, principal) = if let Some(h) = h {
                let Ok((p, a, n)) = paid(&v, &legs, i.qty, h, (target, None)).await else {
                    continue;
                };
                (p, Some(a), n)
            } else {
                let Ok(p) = route_calc::eval(
                    &q.ctx,
                    &legs,
                    i.side,
                    i.qty,
                    pool_tax::coin_keep(&legs, i.side, &i.taxes),
                ) else {
                    continue;
                };
                let Some(first) = p.hops.first() else {
                    continue;
                };
                let principal = first.swapped.amount_in;
                (p, None, principal)
            };
            let Ok(mut quote) = costs::quote(
                &q.ctx,
                &i.asset,
                i.side,
                i.qty,
                &[path],
                RouteMode::Direct,
                &i.taxes,
                i.gas,
                &names,
            ) else {
                continue;
            };
            if let Some(route) = quote.route.as_mut() {
                for leg in &mut route.legs {
                    for hop in &mut leg.hops {
                        hop.fee_mode = Some(if moto {
                            RouterFeeMode::MotoswapPath { target, principal }
                        } else {
                            RouterFeeMode::FluidHop { target, principal }
                        });
                    }
                }
            }
            out.push((quote, approval));
        }
    }
    out
}

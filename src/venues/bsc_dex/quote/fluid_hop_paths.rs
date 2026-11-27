//! Обходит индекс составных маршрутов и проверяет непрерывность, состояние и комиссии каждой ноги.
use super::{graph_legs, pool_tax, route_calc::Leg, routing::Input, select, via::Via};
use crate::{
    core::types::Side,
    gateway::errors::GatewayResult,
    venues::bsc_dex::pools::{fluid_dex_lite as lite, registry::Entry},
};
use alloy_primitives::Address;
use std::{collections::VecDeque, sync::Arc};

pub(super) fn sides(v: &Via<'_>) -> (Address, Address) {
    if v.side == Side::Buy {
        (v.usdt, v.coin)
    } else {
        (v.coin, v.usdt)
    }
}

pub(super) fn routes(v: &Via<'_>, i: &Input) -> Vec<Vec<Arc<Entry>>> {
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

pub(super) async fn legs(v: &Via<'_>, entries: &[Arc<Entry>]) -> GatewayResult<Vec<Leg>> {
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

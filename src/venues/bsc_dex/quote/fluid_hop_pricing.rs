//! Переоценивает выбранный составной маршрут и сравнивает оплачиваемые варианты по фактическим издержкам.
pub(super) use super::fluid_hop_paths::sides;
use super::fluid_hop_paths::{legs, routes};
use super::{
    Quoting, costs, fluid_hop_sizing::paid, path::Path, pool_tax, route_calc, routing::Input,
    steps::Names, v2_execution::OptionQuote, via::Via, white_state,
};
use crate::{
    core::types::{RouteHop, RouteMode, RouterFeeMode},
    gateway::errors::GatewayResult,
    venues::bsc_dex::{gas_ticker, pools::fluid_dex_lite as lite},
};
use alloy_primitives::{Address, U256};
use rust_decimal::Decimal;

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

pub(in crate::venues::bsc_dex) async fn candidates(
    q: &Quoting,
    i: &Input,
    native: bool,
) -> Vec<OptionQuote> {
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

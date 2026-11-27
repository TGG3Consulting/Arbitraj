//! Public price levels in raw units. The final stage requests the exact signed size from each selected maker.
use super::client::{Maker, Query};
use crate::{
    core::types::{Asset, Cost, CostKind, DexRoute, Quote, RouteMode, Side},
    gateway::errors::GatewayResult,
    venues::bsc_dex::{gas_ticker, hashflow, num, parts::Parts, quote::direct},
};
use alloy_primitives::{Address, U256};
use futures_util::{StreamExt, stream};
use rust_decimal::Decimal;

pub(super) async fn level(p: &Parts, q: Query) -> GatewayResult<(U256, U256)> {
    let c = &p.ctx;
    let peers = Maker::endpoints(c);
    let jobs: Vec<_> = peers.iter().map(|m| m.level(c, q)).collect();
    let mut responses = stream::iter(jobs).buffer_unordered(4);
    let mut best = None;
    let mut last = None;
    let deadline = tokio::time::Instant::now() + c.timeout()?;
    while let Ok(Some(row)) = tokio::time::timeout_at(deadline, responses.next()).await {
        match row {
            Ok(Some(v))
                if best.is_none_or(
                    |b: (U256, U256)| if q.exact_out { v.0 < b.0 } else { v.1 > b.1 },
                ) =>
            {
                best = Some(v)
            }
            Err(e) => last = Some(e),
            _ => {}
        }
    }
    best.ok_or_else(|| {
        last.unwrap_or_else(|| super::no(c, "AirSwap: нет публичного уровня на заданный объём"))
    })
}
pub(super) async fn compare(
    p: &Parts,
    a: &Asset,
    s: Side,
    qty: Decimal,
    pair: (Address, Address),
    old: GatewayResult<Quote>,
) -> GatewayResult<Quote> {
    let c = &p.ctx;
    let buy = s == Side::Buy;
    let (Ok(coin), Ok(cash)) = tokio::join!(
        p.tokens.decimals(c, if buy { pair.1 } else { pair.0 }),
        p.tokens.decimals(c, if buy { pair.0 } else { pair.1 })
    ) else {
        return old;
    };
    let Some(amount) = direct::ceil_units(qty, coin) else {
        return old;
    };
    let Ok((spent, received)) = level(
        p,
        Query {
            input: pair.0,
            output: pair.1,
            amount,
            exact_out: buy,
        },
    )
    .await
    else {
        return old;
    };
    if num::to_decimal(if buy { received } else { spent }, coin) != Some(qty) {
        return old;
    }
    let Some(price) =
        num::to_decimal(if buy { spent } else { received }, cash).and_then(|v| v.checked_div(qty))
    else {
        return old;
    };
    let (Ok(budget), Ok(ticker)) = (c.dec("airswap.gas_budget_native"), gas_ticker::read(c)) else {
        return old;
    };
    let q = Quote {
        venue: c.venue(),
        asset: a.clone(),
        side: s,
        qty,
        fillable_qty: qty,
        avg_price: price,
        costs: vec![Cost::new(CostKind::Gas, budget, ticker, false)],
        route: Some(DexRoute {
            mode: RouteMode::RfqIndicative,
            legs: vec![],
            block: 0,
            gas_units: 0,
            executable: false,
            exec_note: Some(super::LABEL.into()),
        }),
        signal_route: None,
        at: chrono::Utc::now(),
        pick: Some(super::LABEL.into()),
    };
    if let Ok(baseline) = &old
        && baseline.fillable_qty == qty
    {
        let (Some(ours), Some(theirs)) = tokio::join!(
            hashflow::routing::total(p, &q),
            hashflow::routing::total(p, baseline)
        ) else {
            return old;
        };
        if if buy { ours >= theirs } else { ours <= theirs } {
            return old;
        }
    }
    Ok(q)
}

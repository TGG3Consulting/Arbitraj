//! Public price levels in raw units. Keep the maker that supplied each price for the final request.
use super::client::{Maker, Query};
use crate::{
    core::types::{Asset, Cost, CostKind, DexRoute, Quote, RouteMode, Side},
    gateway::errors::GatewayResult,
    venues::bsc_dex::{gas_ticker, hashflow, num, parts::Parts, quote::direct},
};
use alloy_primitives::{Address, U256};
use futures_util::{StreamExt, stream};
use rust_decimal::Decimal;

#[derive(Clone)]
pub(super) struct Offer {
    pub maker: Maker,
    pub paid: U256,
    pub net: U256,
}
async fn offers(p: &Parts, q: Query) -> GatewayResult<Vec<Offer>> {
    let c = &p.ctx;
    let jobs = Maker::endpoints(c).into_iter().map(|m| async move {
        let row = m.level(c, q).await?;
        Ok::<_, crate::gateway::errors::GatewayError>(row.map(|(paid, net)| Offer {
            maker: m,
            paid,
            net,
        }))
    });
    let mut stream = stream::iter(jobs).buffer_unordered(4);
    let deadline = tokio::time::Instant::now() + c.timeout()?;
    let mut rows = Vec::new();
    let mut last = None;
    while let Ok(Some(row)) = tokio::time::timeout_at(deadline, stream.next()).await {
        match row {
            Ok(Some(r)) => rows.push(r),
            Err(e) => last = Some(e),
            _ => {}
        }
    }
    rows.sort_by(|a, b| {
        if q.exact_out {
            a.paid.cmp(&b.paid)
        } else {
            b.net.cmp(&a.net)
        }
    });
    if rows.is_empty() {
        return Err(last
            .unwrap_or_else(|| super::no(c, "AirSwap: нет публичного уровня на заданный объём")));
    }
    Ok(rows)
}
pub(super) async fn level(p: &Parts, q: Query) -> GatewayResult<(U256, U256)> {
    let offers = offers(p, q).await?;
    Ok((offers[0].paid, offers[0].net))
}
#[derive(Clone)]
pub(super) struct Level {
    pub query: Query,
    pub offers: Vec<Offer>,
    pub decimals: (u8, u8),
}
pub(super) async fn observe(
    p: &Parts,
    s: Side,
    qty: Decimal,
    pair: (Address, Address),
    wait: bool,
) -> Option<Level> {
    use crate::venues::bsc_dex::quote::rfq_feed::identity;
    let c = &p.ctx;
    let buy = s == Side::Buy;
    let key = ((identity(c)?, pair.0, pair.1), buy, qty);
    let parts = p.clone();
    p.airswap
        .prices
        .observe(key, c.timeout().ok()?, wait, async move {
            let c = &parts.ctx;
            let (di, dout) = tokio::try_join!(
                parts.tokens.decimals(c, pair.0),
                parts.tokens.decimals(c, pair.1)
            )
            .ok()?;
            let amount = direct::ceil_units(qty, if buy { dout } else { di })?;
            let query = Query {
                input: pair.0,
                output: pair.1,
                amount,
                exact_out: buy,
            };
            Some(Level {
                query,
                offers: offers(&parts, query).await.ok()?,
                decimals: (di, dout),
            })
        })
        .await
}
pub(super) async fn priced(
    p: &Parts,
    a: &Asset,
    s: Side,
    qty: Decimal,
    l: &Level,
    offer: &Offer,
    old: Option<&Quote>,
) -> Option<Quote> {
    let c = &p.ctx;
    let buy = s == Side::Buy;
    let (spent, received) = (offer.paid, offer.net);
    let (coin, cash) = if buy {
        (l.decimals.1, l.decimals.0)
    } else {
        l.decimals
    };
    if num::to_decimal(if buy { received } else { spent }, coin) != Some(qty) {
        return None;
    }
    let price = num::to_decimal(if buy { spent } else { received }, cash)?.checked_div(qty)?;
    let (Ok(budget), Ok(ticker)) = (c.dec("airswap.gas_budget_native"), gas_ticker::read(c)) else {
        return None;
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
    if let Some(baseline) = old
        && baseline.fillable_qty == qty
    {
        let (Some(ours), Some(theirs)) = tokio::join!(
            hashflow::routing::total(p, &q),
            hashflow::routing::total(p, baseline)
        ) else {
            return None;
        };
        if if buy { ours >= theirs } else { ours <= theirs } {
            return None;
        }
    }
    Some(q)
}

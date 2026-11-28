//! Оценка объёма по книге выбранного строителя и перепроверка кошелька, предела и полной стоимости в его слоте.
use super::super::{hashflow, parts::Parts, send::tx_1559::Fees};
use super::{firm::firm, indicative::candidate, markets::Books, slot::Proof};
use crate::core::types::{Asset, Quote, RouteMode, Side};
use crate::gateway::errors::{ErrorCode, GatewayResult};
use alloy::primitives::U256;
use rust_decimal::Decimal;
use std::{sync::Arc, time::Duration};

pub(super) struct Prepared {
    pub book: Arc<Books>,
    pub proof: Proof,
    pub base: U256,
    pub fees: Fees,
    pub gas_limit: u64,
    pub spent: U256,
    pub received: U256,
    pub decs: (u8, u8),
}
#[path = "tickets.rs"]
mod tickets;
pub(super) use tickets::Tickets;
pub(in crate::venues::bsc_dex) fn active(p: &Parts) -> bool {
    p.ctx.chain_id() == Some(1) && !p.slot_markets.current(p).is_empty()
}
pub(in crate::venues::bsc_dex) fn breaks(
    p: &Parts,
    asset: &Asset,
    side: Side,
    max: Decimal,
) -> Vec<Decimal> {
    let Some(coin) = asset.contract else {
        return Vec::new();
    };
    let mut points = std::collections::BTreeSet::new();
    for book in p.slot_markets.current(p) {
        if crate::gateway::quote_stage::settlement() && book.prop.is_none() {
            continue;
        }
        if let Some(side) = book.sides.get(&(coin, side == Side::Buy)) {
            points.extend(side.breaks(max));
        }
    }
    points.into_iter().collect()
}
/// Сравнение двух полных выплат одной физической кассы после перевода газа в эту же валюту.
fn improves(side: Side, value: Decimal, baseline: Option<Decimal>) -> bool {
    baseline.is_none_or(|old| {
        if side == Side::Buy {
            value < old
        } else {
            value > old
        }
    })
}
pub(in crate::venues::bsc_dex) async fn consider(
    p: &Parts,
    asset: &Asset,
    side: Side,
    qty: Decimal,
    baseline: GatewayResult<Quote>,
) -> GatewayResult<Quote> {
    if p.ctx.chain_id() != Some(1)
        || baseline
            .as_ref()
            .is_err_and(|e| e.code != ErrorCode::NoRoute)
    {
        return baseline;
    }
    let mixed = crate::gateway::quote_stage::settlement();
    let candidates: Vec<_> = p
        .slot_markets
        .current(p)
        .into_iter()
        .filter(|b| !mixed || b.prop.is_some())
        .filter_map(|b| candidate(p, b, asset, side, qty))
        .collect();
    if candidates.is_empty() {
        return baseline;
    }
    let old = if let Ok(b) = &baseline
        && b.fillable_qty == qty
        && b.qty == qty
        && (!mixed
            || crate::gateway::quote_stage::settlement_quote_supported(
                b,
                crate::gateway::quote_stage::is_final(),
            ))
        && (!crate::gateway::quote_stage::is_final()
            || b.route.as_ref().is_none_or(|r| r.executable))
    {
        let cost = hashflow::routing::total(p, b).await;
        if cost.is_none() {
            return baseline;
        }
        cost
    } else {
        None
    };
    let final_quote = crate::gateway::quote_stage::is_final();
    if final_quote && p.ctx.account.is_none() {
        return baseline;
    }
    let options = futures_util::future::join_all(candidates.into_iter().map(|c| async move {
        if final_quote {
            if let Some(base) = old {
                let buy = side == Side::Buy;
                let coin = if buy { c.output } else { c.input };
                // An actual measured level in this exact frame fixes the quoted gross for this amount.
                // A zero gas allowance for this comparison is already maximally favourable to the candidate.
                if c.book.sides.get(&(coin, buy)).is_some_and(|levels| {
                    levels
                        .0
                        .iter()
                        .any(|(size, gross)| *size == qty && !improves(side, *gross, Some(base)))
                }) {
                    return None;
                }
                if super::prop_levels::Quoted::book(&c.book).is_none()
                    && !improves(
                        side,
                        hashflow::routing::total(p, &c.quote).await?,
                        Some(base),
                    )
                {
                    return None;
                }
            }
            let timeout = Duration::from_millis(c.book.config.timeout);
            let (p, q) = tokio::time::timeout(timeout, firm(p, c)).await.ok()??;
            Some((Some(p), q))
        } else {
            Some((None, c.quote))
        }
    }))
    .await;
    let mut best = baseline;
    let mut value = old;
    let mut ticket: Option<Prepared> = None;
    for (prepared, mut q) in options.into_iter().flatten() {
        if mixed && let Some(route) = q.route.as_mut() {
            route.mode = RouteMode::PropSettlementSlot(None);
            route.exec_note = Some("PropAMM: собственный однократный слот денежного шага".into());
        }
        let score = match &prepared {
            Some(t) => super::firm::value(p, &q, t).await,
            None => hashflow::routing::total(p, &q).await,
        };
        let Some(net) = score else {
            continue;
        };
        if !improves(side, net, value) {
            if Some(net) != value {
                continue;
            }
            let (Some(prior), Some(new), Ok(old_quote)) = (&ticket, &prepared, &best) else {
                continue;
            };
            let (a, b) = tokio::join!(
                super::firm::capital(p, &q, new),
                super::firm::capital(p, old_quote, prior)
            );
            if !matches!((a,b),(Some(x),Some(y)) if x<y) {
                continue;
            }
        }
        if prepared
            .as_ref()
            .is_some_and(|v| !super::slot_config::timely(p, &v.book))
        {
            continue;
        }
        best = Ok(q);
        value = Some(net);
        ticket = prepared;
    }
    if let Some(prepared) = ticket
        && let Ok(q) = best.as_mut()
        && let Some(route) = q.route.as_mut()
    {
        let id = p.slot_markets.tickets.save(prepared, mixed);
        route.mode = if mixed {
            RouteMode::PropSettlementSlot(Some(id))
        } else {
            RouteMode::Slot(id)
        };
    }
    best
}

#[cfg(test)]
#[path = "slot_selection_tests.rs"]
mod slot_selection_tests;

#[cfg(test)]
#[path = "settlement_mode_tests.rs"]
mod settlement_mode_tests;

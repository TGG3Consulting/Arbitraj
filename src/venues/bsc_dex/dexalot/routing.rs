//! Сравнение оплачиваемых котировок по расходам, газу и бюджету в штатном графе.
use super::LABEL;
pub(in crate::venues::bsc_dex) use super::routing_book::{Book, Ticket, available};
use crate::{
    core::types::{Asset, Quote, Side},
    gateway::{
        errors::{ErrorCode, GatewayResult},
        quote_stage,
    },
    venues::bsc_dex::{
        hashflow, num,
        parts::Parts,
        quote::{wallet_rfq, white_state},
        send,
    },
};
use alloy_primitives::U256;
use rust_decimal::Decimal;

/// A simulated rebate can disappear at inclusion. Compare the calldata's maximum debit to the budget.
pub(super) fn within_input(authorized: U256, decimals: u8, budget: Decimal) -> bool {
    num::to_units(budget, decimals).is_some_and(|limit| authorized <= limit)
}
pub(in crate::venues::bsc_dex) async fn consider(
    p: &Parts,
    a: &Asset,
    s: Side,
    qty: Decimal,
    old: GatewayResult<Quote>,
) -> GatewayResult<Quote> {
    if !available(p, a)
        || quote_stage::settlement()
        || qty <= Decimal::ZERO
        || old.as_ref().is_err_and(|e| e.code != ErrorCode::NoRoute)
    {
        return old;
    }
    let Some(pair) = p.dexalot.pair(p, a, s) else {
        return old;
    };
    let c = &p.ctx;

    let buy = s == Side::Buy;
    let matching = || {
        let mut candidates = p.dexalot.candidates(p, a, s).unwrap_or_default();
        candidates.retain(|t| {
            let (v, d) = if buy {
                (t.requested_output.unwrap_or(t.received), t.dout)
            } else {
                (t.spent, t.di)
            };
            num::to_decimal(v, d) == Some(qty)
        });
        candidates
    };
    let mut candidates = matching();
    if candidates.is_empty() {
        let Ok(wait) = c.timeout() else { return old };
        match tokio::time::timeout(
            wait,
            Box::pin(super::automatic::evaluate(
                p,
                a,
                s,
                qty,
                pair,
                old.as_ref().ok(),
            )),
        )
        .await
        {
            Ok(super::automatic::Next::Indicative(q)) => return Ok(*q),
            Ok(super::automatic::Next::Reserved) => candidates = matching(),
            _ => return old,
        }
        if candidates.is_empty() {
            return old;
        }
    }
    let Ok(h) = white_state::head(c, "latest").await else {
        return old;
    };
    let mut cutoff = if let Ok(q) = &old
        && q.fillable_qty == qty
        && (!quote_stage::is_final() || q.route.as_ref().is_some_and(|r| r.executable))
    {
        let Some(current) = hashflow::routing::total(p, q).await else {
            return old;
        };
        Some(current)
    } else {
        None
    };
    let mut best = old;
    for t in candidates {
        let (coin, cash, cd, sd) = if buy {
            (
                t.requested_output.unwrap_or(t.received),
                t.spent,
                t.dout,
                t.di,
            )
        } else {
            (t.spent, t.received, t.di, t.dout)
        };
        let budget = if buy {
            quote_stage::buy_budget(c.place.id, pair.0)
        } else {
            Some(qty)
        };
        if budget.is_some_and(|limit| !within_input(t.proof.amount, t.di, limit))
            || num::to_decimal(coin, cd) != Some(qty)
            || send::select(c, &p.stats)
                .ok()
                .is_none_or(|channels| send::tip(&channels).is_some())
        {
            continue;
        }
        let gas = if quote_stage::is_final() {
            let Ok((spent, net, g)) = t.proof.measure(c, h, &t.data).await else {
                continue;
            };
            if (spent, net) != (t.spent, t.received) {
                continue;
            }
            g
        } else {
            t.gas
        };
        let Some(q) = wallet_rfq::quote(
            p,
            a,
            s,
            qty,
            wallet_rfq::Terms {
                call: &t.proof.call,
                data: &t.data,
                cash,
                cash_dec: sd,
                block: h.number.to(),
                gas,
                gas_setting: "dexalot.gas_budget_native",
                label: LABEL,
            },
        )
        .await
        else {
            continue;
        };
        let Some(cost) = hashflow::routing::total(p, &q).await else {
            continue;
        };
        if cutoff.is_none_or(|old| if buy { cost < old } else { cost > old }) {
            cutoff = Some(cost);
            best = Ok(q)
        }
    }
    best
}

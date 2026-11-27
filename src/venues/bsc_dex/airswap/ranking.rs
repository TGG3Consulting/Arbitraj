//! Выбор исполнимого предложения по итоговой стоимости с газом и верхней границей расхода.
use super::{
    LABEL,
    book::{Ticket, within_input},
};
use crate::{
    core::types::{Asset, Quote, Side},
    gateway::{errors::GatewayResult, quote_stage},
    venues::bsc_dex::{
        hashflow, num,
        parts::Parts,
        quote::{wallet_rfq, white_state},
        send,
    },
};
use alloy_primitives::Address;
use rust_decimal::Decimal;
use std::sync::Arc;

pub(super) async fn best(
    p: &Parts,
    a: &Asset,
    s: Side,
    qty: Decimal,
    input: Address,
    old: GatewayResult<Quote>,
    candidates: Vec<Arc<Ticket>>,
) -> GatewayResult<Quote> {
    let c = &p.ctx;
    let buy = s == Side::Buy;
    if candidates.is_empty() {
        return old;
    }
    let Ok(h) = white_state::head(c, "latest").await else {
        return old;
    };
    let mut cutoff = if let Ok(q) = &old
        && q.fillable_qty == qty
        && (!quote_stage::is_final() || q.route.as_ref().is_some_and(|r| r.executable))
    {
        let Some(value) = hashflow::routing::total(p, q).await else {
            return old;
        };
        Some(value)
    } else {
        None
    };
    let mut best = old;
    for t in candidates {
        let (coin, cash, cd, sd) = if buy {
            (t.received, t.spent, t.dout, t.di)
        } else {
            (t.spent, t.received, t.di, t.dout)
        };
        let budget = if buy {
            quote_stage::buy_budget(c.place.id, input)
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
                gas_setting: "airswap.gas_budget_native",
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

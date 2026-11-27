//! Сравнивает публичную оценку с другими маршрутами и запрашивает firm только на заключительном этапе.
pub(super) use super::automatic_feed::Feed;
use super::{automatic_feed::ready, firm};
use crate::{
    core::types::{Asset, Cost, CostKind, DexRoute, Quote, RouteMode, Side},
    gateway::{quote_stage, source_quote::FirmTerms},
    venues::bsc_dex::{gas_ticker, hashflow, num, parts::Parts, quote::rfq_feed::identity, wallet},
};
use alloy_primitives::{Address, U256};
use rust_decimal::Decimal;
use std::time::Duration;
use tokio::time::Instant;

pub(super) enum Next {
    Keep,
    Indicative(Box<Quote>),
    Reserved,
}
pub(super) async fn evaluate(
    p: &Parts,
    a: &Asset,
    s: Side,
    qty: Decimal,
    pair: (Address, Address),
    baseline: Option<&Quote>,
) -> Next {
    let c = &p.ctx;
    if !ready(c) {
        return Next::Keep;
    }
    let buy = s == Side::Buy;
    let final_stage = quote_stage::is_final();
    let Some(view) = p
        .dexalot
        .feed
        .observe(c, pair, buy, qty, baseline.is_none())
        .await
    else {
        return Next::Keep;
    };
    let (debit, credit) = view.decs;
    let Some(cash) = num::to_decimal(
        if buy { view.spent } else { view.net },
        if buy { debit } else { credit },
    ) else {
        return Next::Keep;
    };
    let Some(rate) = cash.checked_div(qty) else {
        return Next::Keep;
    };
    let (Ok(gas), Ok(ticker)) = (c.dec("dexalot.gas_budget_native"), gas_ticker::read(c)) else {
        return Next::Keep;
    };
    let candidate = Quote {
        venue: c.venue(),
        asset: a.clone(),
        side: s,
        qty,
        fillable_qty: qty,
        avg_price: rate,
        costs: vec![Cost::new(CostKind::Gas, gas, ticker, false)],
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
    if let Some(old) = baseline.filter(|b| b.fillable_qty == qty) {
        let (Some(ours), Some(theirs)) = tokio::join!(
            hashflow::routing::total(p, &candidate),
            hashflow::routing::total(p, old)
        ) else {
            return Next::Keep;
        };
        if if buy { ours >= theirs } else { ours <= theirs } {
            return Next::Keep;
        }
        if !final_stage && old.route.as_ref().is_some_and(|r| r.executable) {
            return Next::Keep;
        }
    }
    let max = if buy {
        match quote_stage::buy_budget(c.place.id, pair.0) {
            Some(v) => {
                let Some(raw) = num::to_units(v, debit) else {
                    return Next::Keep;
                };
                raw
            }
            None => view.spent,
        }
    } else {
        view.order.qty
    };
    if view.spent > max {
        return Next::Keep;
    }
    if !final_stage {
        return Next::Indicative(Box::new(candidate));
    }
    let (Ok(who), Ok(wallet), Some(id), Ok(wait), Ok(ms)) = (
        wallet::signer::address(c),
        c.bound(),
        identity(c),
        c.timeout(),
        c.int("dexalot.min_validity_ms"),
    ) else {
        return Next::Keep;
    };
    let key = ((id, pair.0, pair.1), who);
    if !p
        .dexalot
        .feed
        .pending
        .acquire(key, Instant::now(), wait.max(Duration::from_millis(ms)))
    {
        return Next::Keep;
    }
    let terms = FirmTerms {
        wallet: wallet.to_string(),
        minimum_raw: if buy { view.order.qty } else { U256::ONE }.to_string(),
        maximum_raw: buy.then(|| max.to_string()),
        allow_trade: true,
        fee_tenths_bps: None,
    };
    let result = tokio::time::timeout(
        wait,
        Box::pin(firm::report(
            p,
            view.order,
            terms,
            Some(who),
            view.decs,
            Some(view.spent),
        )),
    )
    .await;
    if let Ok(Ok(r)) = result
        && let Some(expiry) = r.expires
    {
        p.dexalot.feed.pending.hold(key, expiry);
        Next::Reserved
    } else {
        Next::Keep
    }
}

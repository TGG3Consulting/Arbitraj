use super::{LABEL, Limits, client::Query, enabled, options, receipt::Saved, timely};
use crate::{
    core::types::{Asset, Quote, Side},
    gateway::{
        errors::{ErrorCode, GatewayResult},
        quote_stage,
    },
    venues::bsc_dex::{
        hashflow, num,
        parts::Parts,
        quote::{direct, wallet_rfq, white_state},
        send, wallet,
    },
};
use alloy_primitives::{Address, B256, Bytes, U256};
use rust_decimal::Decimal;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
pub(in crate::venues::bsc_dex) struct Ticket {
    pub(super) proof: Saved,
    pub(super) data: Bytes,
    pub(super) spent: U256,
    pub(super) received: U256,
    pub(super) gas: u64,
    pub(super) generation: u64,
    pub(super) di: u8,
    pub(super) dout: u8,
}
type Tickets = HashMap<(Address, B256), Arc<Ticket>>;
#[derive(Clone, Default)]
pub(in crate::venues::bsc_dex) struct Book(Arc<Mutex<Tickets>>);
impl Book {
    pub(super) fn put(&self, t: Ticket) {
        let mut rows = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let now = chrono::Utc::now().timestamp().max(0) as u64;
        rows.retain(|_, v| v.proof.expiry > now);
        rows.insert((t.proof.call.payer, t.proof.call.calldata), Arc::new(t));
    }
    pub(in crate::venues::bsc_dex) fn take(&self, who: Address, id: B256) -> Option<Arc<Ticket>> {
        self.0.lock().ok()?.remove(&(who, id))
    }
    fn pair(&self, p: &Parts, a: &Asset, s: Side) -> Option<(Address, Address)> {
        let coin = a.contract?;
        let cash = p.tokens.usdt(&p.ctx).ok()?;
        Some(if s == Side::Buy {
            (cash, coin)
        } else {
            (coin, cash)
        })
    }
    fn candidates(&self, p: &Parts, a: &Asset, s: Side) -> Option<Vec<Arc<Ticket>>> {
        let pair = self.pair(p, a, s)?;
        let ctx = &p.ctx;
        let who = wallet::signer::address(ctx).ok()?;
        Some(
            self.0
                .lock()
                .ok()?
                .values()
                .filter(|v| {
                    (v.proof.input, v.proof.output) == pair
                        && v.proof.call.payer == who
                        && v.generation == ctx.settings.generation()
                        && timely(ctx, v.proof.expiry)
                })
                .cloned()
                .collect(),
        )
    }
    pub(in crate::venues::bsc_dex) fn breaks(
        &self,
        p: &Parts,
        a: &Asset,
        s: Side,
        max: Decimal,
    ) -> Vec<Decimal> {
        if !enabled(&p.ctx) {
            return vec![];
        }
        self.candidates(p, a, s)
            .unwrap_or_default()
            .iter()
            .filter_map(|v| {
                let (raw, dec) = if s == Side::Buy {
                    (v.received, v.dout)
                } else {
                    (v.spent, v.di)
                };
                num::to_decimal(raw, dec).filter(|v| *v > Decimal::ZERO && *v <= max)
            })
            .collect()
    }
}
pub(in crate::venues::bsc_dex) fn available(p: &Parts) -> bool {
    enabled(&p.ctx)
        && p.ctx
            .settings
            .list("airswap.makers", p.ctx.at())
            .is_ok_and(|v| !v.is_empty())
        && !super::profile::Profile::candidates(&p.ctx).is_empty()
}
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
    if !available(p)
        || quote_stage::settlement()
        || qty <= Decimal::ZERO
        || old.as_ref().is_err_and(|e| e.code != ErrorCode::NoRoute)
    {
        return old;
    }
    let Some(pair) = p.airswap.pair(p, a, s) else {
        return old;
    };
    let c = &p.ctx;
    let Ok(who) = wallet::signer::address(c) else {
        return old;
    };
    let buy = s == Side::Buy;
    let mut candidates = p.airswap.candidates(p, a, s).unwrap_or_default();
    candidates.retain(|t| {
        let (v, d) = if buy {
            (t.received, t.dout)
        } else {
            (t.spent, t.di)
        };
        num::to_decimal(v, d) == Some(qty)
    });
    if candidates.is_empty() && !quote_stage::is_final() {
        return super::indicative::compare(p, a, s, qty, pair, old).await;
    }
    let Ok(h) = white_state::head(c, "latest").await else {
        return old;
    };
    if candidates.is_empty() {
        let Ok(dec) = p
            .tokens
            .decimals(c, if buy { pair.1 } else { pair.0 })
            .await
        else {
            return old;
        };
        let Some(raw) = direct::ceil_units(qty, dec) else {
            return old;
        };
        let maximum = if buy {
            match quote_stage::buy_budget(c.place.id, pair.0) {
                Some(b) => {
                    let Ok(d) = p.tokens.decimals(c, pair.0).await else {
                        return old;
                    };
                    let Some(cap) = num::to_units(b, d) else {
                        return old;
                    };
                    cap
                }
                None => U256::MAX,
            }
        } else {
            raw
        };
        let query = Query {
            input: pair.0,
            output: pair.1,
            amount: raw,
            exact_out: buy,
        };
        let limits = Limits {
            minimum: if buy { raw } else { U256::ONE },
            maximum,
        };
        let Ok(rows) = options(p, query, limits, who, h).await else {
            return old;
        };
        for t in rows {
            p.airswap.put(t)
        }
        candidates = p.airswap.candidates(p, a, s).unwrap_or_default();
    }
    let mut cutoff = if let Ok(q) = &old
        && q.fillable_qty == qty
        && (!quote_stage::is_final() || q.route.as_ref().is_some_and(|r| r.executable))
    {
        hashflow::routing::total(p, q).await
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

//! Сравнение только действующих предложений, которые владелец заранее разрешил для торговли.
use super::{LABEL, current, enabled, receipt::Saved};
use crate::{
    core::types::{Asset, Quote, Side},
    gateway::{
        errors::{ErrorCode, GatewayResult},
        quote_stage,
    },
    venues::bsc_dex::{
        hashflow, num, parts::Parts, quote::white_state, send, wallet,
    },
};
use alloy_primitives::{Address, B256, Bytes, U256};
use rust_decimal::Decimal;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

pub(in crate::venues::bsc_dex) struct Ticket {
    pub proof: Saved,
    pub data: Bytes,
    pub received: U256,
    pub gas: u64,
    pub generation: u64,
    pub di: u8,
    pub dout: u8,
}
type Tickets = HashMap<(Address, B256), Arc<Ticket>>;
#[derive(Clone, Default)]
pub(in crate::venues::bsc_dex) struct Book(Arc<Mutex<Tickets>>);
impl Book {
    pub(super) fn put(&self, t: Ticket) {
        let mut entries = self.0.lock().unwrap_or_else(|e| e.into_inner());
        entries.retain(|_, v| current(v.proof.expiry));
        entries.insert((t.proof.call.payer, t.proof.call.calldata), Arc::new(t));
    }
    pub(in crate::venues::bsc_dex) fn take(&self, who: Address, id: B256) -> Option<Arc<Ticket>> {
        self.0.lock().ok()?.remove(&(who, id))
    }
    pub(in crate::venues::bsc_dex) fn has(&self, a: &Asset) -> bool {
        a.contract.is_some_and(|coin| {
            self.0.lock().is_ok_and(|e| {
                e.values().any(|v| {
                    current(v.proof.expiry) && (v.proof.input == coin || v.proof.output == coin)
                })
            })
        })
    }
    pub(in crate::venues::bsc_dex) fn breaks(
        &self,
        p: &Parts,
        a: &Asset,
        s: Side,
        max: Decimal,
    ) -> Vec<Decimal> {
        if !self.has(a) {
            return vec![];
        }
        self.candidates(p, a, s)
            .unwrap_or_default()
            .iter()
            .filter_map(|v| {
                let (raw, dec) = if s == Side::Buy {
                    (v.received, v.dout)
                } else {
                    (v.proof.debit, v.di)
                };
                num::to_decimal(raw, dec).filter(|n| *n > Decimal::ZERO && *n <= max)
            })
            .collect()
    }
    fn candidates(&self, p: &Parts, asset: &Asset, s: Side) -> Option<Vec<Arc<Ticket>>> {
        let coin = asset.contract?;
        let cash = p.tokens.usdt(&p.ctx).ok()?;
        let pair = if s == Side::Buy {
            (cash, coin)
        } else {
            (coin, cash)
        };
        let who = wallet::signer::address(&p.ctx).ok()?;
        let generation = p.ctx.settings.generation();
        Some(
            self.0
                .lock()
                .ok()?
                .values()
                .filter(|v| {
                    v.proof.call.payer == who
                        && v.generation == generation
                        && current(v.proof.expiry)
                        && (v.proof.input, v.proof.output) == pair
                })
                .cloned()
                .collect(),
        )
    }
}
pub(in crate::venues::bsc_dex) async fn consider(
    p: &Parts,
    asset: &Asset,
    side: Side,
    qty: Decimal,
    old: GatewayResult<Quote>,
) -> GatewayResult<Quote> {
    if !p.textile.has(asset)
        || !enabled(&p.ctx)
        || qty <= Decimal::ZERO
        || quote_stage::settlement()
        || old.as_ref().is_err_and(|e| e.code != ErrorCode::NoRoute)
    {
        return old;
    }
    let Some(choices) = p.textile.candidates(p, asset, side) else {
        return old;
    };
    let buy = side == Side::Buy;
    let mut best = old;
    let mut limit = if let Ok(q) = &best {
        if q.fillable_qty == qty {
            let Some(net) = hashflow::routing::total(p, q).await else {
                return best;
            };
            Some(net)
        } else {
            None
        }
    } else {
        None
    };
    for t in choices {
        let Some(q) = quote(p, asset, side, qty, &t).await else {
            continue;
        };
        let Some(net) = hashflow::routing::total(p, &q).await else {
            continue;
        };
        if limit.is_none_or(|prior| if buy { net < prior } else { net > prior }) {
            limit = Some(net);
            best = Ok(q);
        }
    }
    best
}
async fn quote(p: &Parts, a: &Asset, s: Side, qty: Decimal, t: &Ticket) -> Option<Quote> {
    let ctx = &p.ctx;
    let buy = s == Side::Buy;
    let (coin, cash, cd, sd) = if buy {
        (t.received, t.proof.debit, t.dout, t.di)
    } else {
        (t.proof.debit, t.received, t.di, t.dout)
    };
    if num::to_decimal(coin, cd)? != qty || send::tip(&send::select(ctx, &p.stats).ok()?).is_some()
    {
        return None;
    }
    let head = white_state::head(ctx, "latest").await.ok()?;
    let gas = if quote_stage::is_final() {
        let (received, g) = t.proof.measure(ctx, head, &t.data).await.ok()?;
        if received != t.received {
            return None;
        }
        g
    } else {
        t.gas
    };
    if !current(t.proof.expiry){return None;}
    use crate::venues::bsc_dex::quote::wallet_rfq::{quote,Terms};
    quote(p,a,s,qty,Terms{call:&t.proof.call,data:&t.data,cash,cash_dec:sd,block:head.number.to(),gas,
        gas_setting:"textile.gas_budget_native",label:LABEL}).await
}

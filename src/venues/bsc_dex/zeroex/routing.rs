//! В горячем пути сравниваются только проверенные личные заявки того же кошелька и размера.
use super::{LABEL, current, enabled, receipt::Saved};
use crate::{
    core::types::{Asset, Quote, RouteMode, Side},
    gateway::{
        errors::{ErrorCode, GatewayResult},
        quote_stage,
    },
    venues::bsc_dex::{
        hashflow, num,
        parts::Parts,
        quote::{wallet_rfq, white_state},
        send, wallet,
    },
};
use alloy_primitives::{Address, B256, Bytes, U256};
use rust_decimal::Decimal;
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

pub(in crate::venues::bsc_dex) struct Ticket {
    pub proof: Saved,
    pub data: Bytes,
    pub gas: u64,
    pub generation: u64,
    pub di: u8,
    pub dout: u8,
}
impl Ticket {
    // Ядро планирует гарантированный выход. Возможный избыток фиксируется по квитанции.
    pub(super) fn units(&self, buy: bool) -> (U256, U256, u8, u8) {
        if buy {
            (self.proof.minimum, self.proof.debit, self.dout, self.di)
        } else {
            (self.proof.debit, self.proof.minimum, self.di, self.dout)
        }
    }
}
type Tickets = HashMap<(Address, B256), Arc<Ticket>>;
#[derive(Clone, Default)]
pub(in crate::venues::bsc_dex) struct Book(Arc<BookState>);

#[derive(Default)]
struct BookState {
    tickets: Mutex<Tickets>,
    last_expiry: AtomicU64,
}
impl Book {
    pub(in crate::venues::bsc_dex) fn put(&self, t: Ticket) {
        let mut book = self.0.tickets.lock().unwrap_or_else(|p| p.into_inner());
        book.retain(|_, v| current(v.proof.expiry));
        book.insert((t.proof.call.payer, t.proof.call.calldata), Arc::new(t));
        self.0.last_expiry.store(
            book.values().map(|t| t.proof.expiry).max().unwrap_or(0),
            Ordering::Release,
        );
    }
    pub(in crate::venues::bsc_dex) fn take(&self, who: Address, id: B256) -> Option<Arc<Ticket>> {
        let mut book = self.0.tickets.lock().ok()?;
        let t = book.remove(&(who, id));
        self.0.last_expiry.store(
            book.values().map(|t| t.proof.expiry).max().unwrap_or(0),
            Ordering::Release,
        );
        t
    }
    fn any_live(&self) -> bool {
        let expiry = self.0.last_expiry.load(Ordering::Acquire);
        expiry != 0 && current(expiry)
    }
    pub(in crate::venues::bsc_dex) fn has(&self, a: &Asset) -> bool {
        self.any_live()
            && a.contract.is_some_and(|coin| {
                self.0.tickets.lock().is_ok_and(|b| {
                    b.values().any(|t| {
                        current(t.proof.expiry) && [t.proof.input, t.proof.output].contains(&coin)
                    })
                })
            })
    }
    fn candidates(&self, p: &Parts, a: &Asset, side: Side) -> Option<Vec<Arc<Ticket>>> {
        if !self.has(a) || !enabled(&p.ctx) {
            return None;
        }
        let who = wallet::signer::address(&p.ctx).ok()?;
        let coin = a.contract?;
        let cash = p.tokens.usdt(&p.ctx).ok()?;
        let pair = if side == Side::Buy {
            (cash, coin)
        } else {
            (coin, cash)
        };
        let generation = p.ctx.settings.generation();
        Some(
            self.0
                .tickets
                .lock()
                .ok()?
                .values()
                .filter(|t| {
                    t.proof.call.payer == who
                        && t.generation == generation
                        && current(t.proof.expiry)
                        && (t.proof.input, t.proof.output) == pair
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
        self.candidates(p, a, s)
            .unwrap_or_default()
            .iter()
            .filter_map(|t| {
                let (coin, _, d, _) = t.units(s == Side::Buy);
                num::to_decimal(coin, d).filter(|v| *v > Decimal::ZERO && *v <= max)
            })
            .collect()
    }
}
pub(in crate::venues::bsc_dex) async fn consider(
    p: &Parts,
    a: &Asset,
    side: Side,
    qty: Decimal,
    old: GatewayResult<Quote>,
) -> GatewayResult<Quote> {
    if !p.zeroex.any_live()
        || qty <= Decimal::ZERO
        || quote_stage::settlement()
        || old.as_ref().is_err_and(|e| e.code != ErrorCode::NoRoute)
    {
        return old;
    }
    let Some(orders) = p.zeroex.candidates(p, a, side) else {
        return old;
    };
    let buy = side == Side::Buy;
    let mut best = old;
    let mut best_total = match &best {
        Ok(q) if q.fillable_qty == qty => {
            let Some(v) = hashflow::routing::total(p, q).await else {
                return best;
            };
            Some(v)
        }
        _ => None,
    };
    for t in orders {
        let (coin, cash, cd, sd) = t.units(buy);
        if num::to_decimal(coin, cd) != Some(qty) {
            continue;
        }
        if send::select(&p.ctx, &p.stats)
            .ok()
            .is_none_or(|s| send::tip(&s).is_some())
        {
            continue;
        }
        let Ok(head) = white_state::head(&p.ctx, "latest").await else {
            continue;
        };
        let gas = if quote_stage::is_final() {
            match t.proof.measure(&p.ctx, head, &t.data).await {
                Ok((out, g)) if out >= t.proof.minimum => g,
                _ => continue,
            }
        } else {
            t.gas
        };
        let terms = wallet_rfq::Terms {
            call: &t.proof.call,
            data: &t.data,
            cash,
            cash_dec: sd,
            block: head.number.to(),
            gas,
            gas_setting: "zeroex.gas_budget_native",
            label: LABEL,
        };
        let Some(mut q) = wallet_rfq::quote(p, a, side, qty, terms).await else {
            continue;
        };
        let Some(net) = hashflow::routing::total(p, &q).await else {
            continue;
        };
        if current(t.proof.expiry) && best_total.is_none_or(|v| if buy { net < v } else { net > v })
        {
            if let Some(route) = &mut q.route {
                route.mode = RouteMode::ZeroEx(t.proof.call.calldata);
            }
            best_total = Some(net);
            best = Ok(q);
        }
    }
    best
}

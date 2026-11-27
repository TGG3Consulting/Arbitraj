//! Хранение подписанных котировок и снятие всех заявок с общим nonce.
use super::{enabled, receipt::Saved, timely};
use crate::{
    core::types::{Asset, Side},
    venues::bsc_dex::{num, parts::Parts, quote::rfq_feed, wallet},
};
use alloy_primitives::{Address, B256, Bytes, U256};
use rust_decimal::Decimal;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

pub(in crate::venues::bsc_dex) struct Ticket {
    pub(super) proof: Saved,
    pub(super) signatory: Address,
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
pub(in crate::venues::bsc_dex) struct Book {
    tickets: Arc<Mutex<Tickets>>,
    pub(super) prices: rfq_feed::PricesAt<super::indicative::Level>,
    pub(super) reservations: rfq_feed::Pending,
}
impl Book {
    pub(super) fn put(&self, t: Ticket) {
        let mut rows = self.tickets.lock().unwrap_or_else(|e| e.into_inner());
        let now = chrono::Utc::now().timestamp().max(0) as u64;
        rows.retain(|_, v| v.proof.expiry > now);
        rows.insert((t.proof.call.payer, t.proof.call.calldata), Arc::new(t));
    }
    pub(in crate::venues::bsc_dex) fn take(&self, who: Address, id: B256) -> Option<Arc<Ticket>> {
        let mut rows = self.tickets.lock().ok()?;
        let ticket = rows.remove(&(who, id))?;
        rows.retain(|_, t| {
            (
                t.proof.call.chain,
                t.proof.profile.exchange.address,
                t.signatory,
                t.proof.nonce,
            ) != (
                ticket.proof.call.chain,
                ticket.proof.profile.exchange.address,
                ticket.signatory,
                ticket.proof.nonce,
            )
        });
        Some(ticket)
    }
    pub(super) fn pair(&self, p: &Parts, a: &Asset, s: Side) -> Option<(Address, Address)> {
        let coin = a.contract?;
        let cash = p.tokens.usdt(&p.ctx).ok()?;
        Some(if s == Side::Buy {
            (cash, coin)
        } else {
            (coin, cash)
        })
    }
    pub(super) fn candidates(&self, p: &Parts, a: &Asset, s: Side) -> Option<Vec<Arc<Ticket>>> {
        let pair = self.pair(p, a, s)?;
        let ctx = &p.ctx;
        let who = wallet::signer::address(ctx).ok()?;
        Some(
            self.tickets
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

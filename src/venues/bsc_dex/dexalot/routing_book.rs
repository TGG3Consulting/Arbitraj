//! Хранит подписанные заявки по кошельку и исключает все варианты уже использованного nonce.
use super::{enabled, receipt::Saved, timely};
use crate::{
    core::types::{Asset, Side},
    venues::bsc_dex::{num, parts::Parts, quote::white_state, wallet},
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
    pub(super) requested_output: Option<U256>,
    pub(super) head: white_state::Head,
    pub(super) generation: u64,
    pub(super) di: u8,
    pub(super) dout: u8,
}
type Tickets = HashMap<(Address, B256), Arc<Ticket>>;
#[derive(Clone, Default)]
pub(in crate::venues::bsc_dex) struct Book {
    tickets: Arc<Mutex<Tickets>>,
    pub(super) feed: super::automatic::Feed,
}
impl Book {
    pub(super) fn put(&self, t: Arc<Ticket>) {
        let mut rows = self.tickets.lock().unwrap_or_else(|e| e.into_inner());
        let now = chrono::Utc::now().timestamp().max(0) as u64;
        rows.retain(|_, v| v.proof.expiry > now);
        rows.insert((t.proof.call.payer, t.proof.call.calldata), t);
    }
    pub(in crate::venues::bsc_dex) fn take(&self, who: Address, id: B256) -> Option<Arc<Ticket>> {
        let mut rows = self.tickets.lock().ok()?;
        let order = rows.remove(&(who, id))?;
        rows.retain(|_, t| {
            (
                t.proof.call.chain,
                t.proof.profile.maker.addr,
                t.proof.nonce,
            ) != (
                order.proof.call.chain,
                order.proof.profile.maker.addr,
                order.proof.nonce,
            )
        });
        Some(order)
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
                    (v.requested_output.unwrap_or(v.received), v.dout)
                } else {
                    (v.spent, v.di)
                };
                num::to_decimal(raw, dec).filter(|v| *v > Decimal::ZERO && *v <= max)
            })
            .collect()
    }
}
pub(in crate::venues::bsc_dex) fn available(p: &Parts, asset: &Asset) -> bool {
    if !enabled(&p.ctx) {
        return false;
    }
    let (Some(coin), Ok(cash)) = (asset.contract, p.tokens.usdt(&p.ctx)) else {
        return false;
    };
    (super::automatic_feed::ready(&p.ctx) && p.dexalot.feed.admits(&p.ctx, coin, cash))
        || p.dexalot.tickets.lock().is_ok_and(|rows| {
            rows.values().any(|v| {
                ((v.proof.input, v.proof.output) == (coin, cash)
                    || (v.proof.input, v.proof.output) == (cash, coin))
                    && timely(&p.ctx, v.proof.expiry)
            })
        })
}

//! Билет выбранного ордера и проверка платёжных прав владельца.
use super::{
    Request,
    assets::AssetCode,
    client::{self, Offer},
    profile::Profile,
};
use crate::{
    core::types::Side,
    venues::bsc_dex::{
        ctx::Ctx,
        node::{call::canonical, rpc},
        quote::white_state::Head,
    },
};
use alloy_primitives::{Address, B256, Bytes};
use chrono::Utc;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Config {
    pub p: Profile,
    pub input: AssetCode,
    pub output: AssetCode,
    pub base: String,
    pub bps: u64,
    pub age: u64,
}
impl Config {
    pub fn read(ctx: &Ctx, input: Address, output: Address) -> Option<Self> {
        if !ctx.flag("pancakex.trading").ok()? {
            return None;
        }
        let p = Profile::read(ctx)?;
        let all = ctx.settings.list("pancakex.tokens", ctx.at()).ok()?;
        let asset = |a| {
            let mut hits = all
                .iter()
                .filter_map(|row| AssetCode::parse(row, p.chain))
                .filter(|v| v.address() == a);
            let found = hits.next()?;
            hits.next().is_none().then_some(found)
        };
        let input = asset(input)?;
        let output = asset(output)?;
        let base = ctx.text("pancakex.base_url").ok()?;
        client::url(&base, "order-handler/order")?;
        let bps = ctx.int("pancakex.slippage_bps").ok()?;
        let age = ctx.int("pancakex.max_lifetime_sec").ok()?;
        (output.can_receive() && bps < 10000 && age > 0).then_some(Self {
            p,
            input,
            output,
            base,
            bps,
            age,
        })
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Ticket {
    pub cfg: Config,
    pub req: Request,
    pub offer: Offer,
    pub decimals: (u8, u8),
    pub side: Side,
    pub qty: Decimal,
    pub block: u64,
    pub deadline: u64,
}
impl Ticket {
    pub fn timely(&self, stamp: u64) -> bool {
        stamp < self.deadline && self.deadline - stamp <= self.cfg.age
    }
    pub(crate) async fn eligible(&self, ctx: &Ctx, h: Head) -> Option<()> {
        let q = &self.req;
        let p = &self.cfg.p;
        let terms = self.offer.terms(p, q)?;
        let at = h.selector();
        let (_, funds, unused, cancelled, code, _, _) = tokio::try_join!(
            p.check(ctx, h, &self.offer.version),
            p.funds(ctx, h, q.input, q.owner),
            p.unused(ctx, h, q.owner, terms.nonce),
            canonical(
                ctx,
                &at,
                p.router.address,
                super::profile::cancelledOrdersCall {
                    owner: q.owner,
                    orderHash: terms.order_hash
                }
            ),
            rpc::call::<Bytes>(ctx, "eth_getCode", json!([q.owner, at])),
            self.cfg
                .input
                .verify(ctx, &at, q.owner, self.offer.filler, p.permit.address),
            self.cfg
                .output
                .verify(ctx, &at, self.offer.filler, q.owner, p.router.address)
        )
        .ok()?;
        (funds >= terms.maximum_input
            && unused
            && !cancelled
            && code.is_empty()
            && self.timely(h.timestamp.to())
            && self.timely(Utc::now().timestamp().max(0) as u64))
        .then_some(())
    }
}
#[derive(Clone, Default)]
pub(in crate::venues::bsc_dex) struct Book(Arc<Mutex<HashMap<(Address, B256), Ticket>>>);
impl Book {
    pub(super) fn store(&self, t: Ticket) {
        let mut m = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let now = Utc::now().timestamp().max(0) as u64;
        m.retain(|_, s| s.timely(now));
        m.insert((t.req.owner, t.offer.hash), t);
    }
    pub(super) fn take(&self, who: Address, id: B256) -> Option<Ticket> {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&(who, id))
    }
}

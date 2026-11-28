//! Книга Raindex: подтверждение рыночного приказа штатным контрактом, оплата из собственного ERC20.
//! https://github.com/rainlanguage/raindex
use super::{ctx::Ctx, node::rpc, parts::Parts, quote::white_state};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use alloy_primitives::{Address, B256, Bytes, U256};
use std::sync::Arc;
mod amount;
pub(super) mod book;
mod cash;
mod history;
mod history_store;
mod history_orders;
mod history_candidates;
mod legacy;
mod payment;
mod profile;
pub(super) mod routing;
pub(super) mod settlement;
mod source;
#[cfg(test)]
mod tests;
mod wire;
use amount::Quantity;
use profile::Profile;
pub(super) const LABEL: &str = "Raindex";
pub(super) fn timely(expiry: u64) -> bool {
    chrono::Utc::now().timestamp() < i64::try_from(expiry).unwrap_or(i64::MAX)
}
fn no(ctx: &Ctx, s: &str) -> GatewayError {
    ctx.err(ErrorCode::NoRoute, s)
}
pub(super) fn enabled(ctx: &Ctx) -> bool {
    ctx.flag("raindex.trading").unwrap_or(false)
}
#[derive(Clone)]
struct Leg {
    p: Profile,
    via: Option<cash::State>,
    order: Arc<wire::Order>,
    hash: B256,
    i: usize,
    j: usize,
    input: Address,
    output: Address,
}
impl Leg {
    fn pair(&self) -> (Address, Address) {
        match self.via {
            Some(b) if self.input == b.role.bridge => (b.role.cash, self.output),
            Some(b) => (self.input, b.role.cash),
            None => (self.input, self.output),
        }
    }
    fn target(&self) -> Address {
        self.via.map_or(self.p.module, |b| b.role.wrapper)
    }
    fn decimals(&self, address: Address, p: &Parts) -> Option<u8> {
        if let Some(b) = self.via
            && address == b.role.bridge
        {
            return Some(b.decimals);
        }
        p.tokens.decimals_known(address)
    }
    async fn current(&self, ctx: &Ctx, h: white_state::Head) -> Option<(Quantity, Quantity, B256)> {
        let response:Bytes=rpc::call(ctx,"eth_call",serde_json::json!([{"from":self.p.module,"to":self.p.book,"input":self.order.quote(self.i,self.j)?},h.selector()])).await.ok()?;
        self.order.price(&response)
    }
}
struct Live {
    max_budget: Option<U256>,
    leg: Leg,
    cap: Quantity,
    ratio: Quantity,
}

//! Одна запись кошелька, домена Permit2 и unordered nonce навсегда связывает отправку с денежным шагом.
use super::{Terms, market::Ticket};
use crate::{
    core::types::TradeRequest,
    gateway::{pending::PendingRef, settlement_intent as intent},
    storage::db::Db,
    venues::bsc_dex::ctx::Ctx,
};
use alloy_primitives::{B256, keccak256};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Clone, Serialize, Deserialize)]
pub struct Marker {
    pub key: String,
    pub account: String,
    pub ticket: Ticket,
    pub start: u64,
    pub anchor: B256,
    pub digest: B256,
}
pub fn identity(ctx: &Ctx, r: &TradeRequest) -> Option<String> {
    let chain = ctx.chain_id()?;
    let account = ctx.account.as_deref()?;
    let coin = r.asset.contract?;
    if r.deal_id.is_empty() || r.venue != ctx.venue() {
        return None;
    }
    Some(intent::current_key().unwrap_or_else(|| {
        format!(
            "pancakex/{}",
            keccak256(json!([chain, ctx.place.id, account, coin, r.side, r.deal_id]).to_string())
        )
    }))
}
impl Marker {
    pub fn terms(&self) -> Option<Terms> {
        self.ticket
            .offer
            .terms(&self.ticket.cfg.p, &self.ticket.req)
            .filter(|t| t.digest == self.digest && t.deadline == self.ticket.deadline)
    }
    pub fn pending(&self) -> PendingRef {
        PendingRef {
            client_id: Some(self.digest.to_string()),
            parse: Some(json!({"pancakex":self})),
            ..Default::default()
        }
    }
    pub fn read(p: &PendingRef) -> Option<Self> {
        let m: Self = serde_json::from_value(p.parse.as_ref()?.get("pancakex")?.clone()).ok()?;
        (p.client_id.as_deref()?.parse::<B256>().ok()? == m.terms()?.digest).then_some(m)
    }
    fn slot(&self, t: &Terms) -> String {
        let p = &self.ticket.cfg.p;
        json!([p.chain, p.permit.address, self.ticket.req.owner, t.nonce]).to_string()
    }
    pub fn claim(&self) -> String {
        json!([self.key, self.digest]).to_string()
    }
    pub async fn holder(&self, db: &Db, t: &Terms) -> Result<Option<String>, String> {
        let slot = self.slot(t);
        db.ledger()
            .read(move |c| {
                c.query_row(
                    "SELECT body FROM core_state WHERE kind='pancakex_permit' AND id=?1",
                    [slot],
                    |r| r.get(0),
                )
                .optional()
            })
            .await
            .map_err(|e| e.to_string())
    }
    pub async fn reserve(&self, db: &Db, t: &Terms) -> Result<bool, String> {
        let slot = self.slot(t);
        let claim = self.claim();
        db.ledger().write(move|c|c.execute("INSERT INTO core_state(kind,id,body,updated_at) VALUES ('pancakex_permit',?1,?2,?3) ON CONFLICT(kind,id) DO NOTHING",params![slot,claim,chrono::Utc::now().to_rfc3339()]).map(|n|n==1)).await.map_err(|e|e.to_string())
    }
}

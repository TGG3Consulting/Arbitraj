//! Устойчивые поля принятого к отправке поручения и одна незавершённая swap-подпись на адрес кошелька и роутер Base.
use super::wire::SwapIntent;
use crate::{
    core::types::TradeRequest,
    gateway::{
        errors::{ErrorCode, GatewayResult},
        pending::PendingRef,
        settlement_intent as intent,
    },
    storage::db::Db,
    venues::bsc_dex::ctx::Ctx,
};
use alloy::primitives::{Address, B256, keccak256};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Clone, Serialize, Deserialize)]
pub struct Marker {
    pub key: String,
    pub account: String,
    pub router: Address,
    pub runtime: B256,
    pub relayer: String,
    pub digest: B256,
    pub tokens: Vec<super::tokens::Token>,
    pub order: SwapIntent,
    pub start: u64,
    pub start_hash: B256,
    pub decimals: (u8, u8),
}

pub fn identity(ctx: &Ctx, r: &TradeRequest) -> GatewayResult<String> {
    let account = ctx
        .account
        .as_deref()
        .ok_or_else(|| ctx.err(ErrorCode::NotApplicable, "Ryze: требуется счёт сделки"))?;
    if ctx.chain_id() != Some(super::CHAIN_ID)
        || r.deal_id.is_empty()
        || r.asset.contract.is_none()
        || r.venue != ctx.venue()
    {
        return Err(ctx.err(
            ErrorCode::NotApplicable,
            "Ryze: нужны Base и идентификаторы сделки и актива",
        ));
    }
    Ok(intent::current_key().unwrap_or_else(|| {
        format!(
            "ryze/{}",
            keccak256(
                json!([
                    super::CHAIN_ID,
                    ctx.place.id,
                    account,
                    r.asset.contract,
                    r.side,
                    r.deal_id
                ])
                .to_string()
                .as_bytes()
            )
        )
    }))
}
impl Marker {
    pub fn pending(&self) -> PendingRef {
        PendingRef {
            client_id: Some(self.digest.to_string()),
            parse: Some(json!({"ryze":self})),
            ..Default::default()
        }
    }
    pub fn read(p: &PendingRef) -> Option<Self> {
        let m: Self = serde_json::from_value(p.parse.as_ref()?.get("ryze")?.clone()).ok()?;
        (p.client_id.as_deref()?.parse::<B256>().ok()? == m.digest
            && m.order.digest(super::CHAIN_ID, m.router) == m.digest)
            .then_some(m)
    }
    fn slot(&self) -> String {
        format!("{}/{}/{}", super::CHAIN_ID, self.router, self.order.user)
    }

    pub fn claim(&self) -> String {
        json!([self.key, self.digest]).to_string()
    }

    /// Вызывается только победителем before_send. INSERT может выиграть одно денежное действие, включая конкурирующие процессы.
    pub async fn reserve(&self, db: &Db) -> Result<bool, String> {
        let k = self.slot();
        let id = self.claim();
        let key = self.key.clone();
        db.ledger().write(move|c|{
            let tx=c.transaction()?;
            let now=chrono::Utc::now().to_rfc3339();
            let n=tx.execute("INSERT INTO core_state(kind,id,body,updated_at) VALUES('ryze_swap_nonce',?1,?2,?3) ON CONFLICT(kind,id) DO NOTHING",params![k,id,now])?;
            if n==1{tx.execute("INSERT INTO core_state(kind,id,body,updated_at) VALUES('ryze_dispatch',?1,?2,?3)",params![key,id,now])?;}
            tx.commit()?;Ok(n==1)
        }).await.map_err(|e|e.to_string())
    }
    pub async fn was_reserved(&self, db: &Db) -> Result<bool, String> {
        let key = self.key.clone();
        let claim = self.claim();
        db.ledger().read(move|c|c.query_row("SELECT EXISTS(SELECT 1 FROM core_state WHERE kind='ryze_dispatch' AND id=?1 AND body=?2)",params![key,claim],|r|r.get(0))).await.map_err(|e|e.to_string())
    }
    pub async fn holder(&self, db: &Db) -> Result<Option<String>, String> {
        let k = self.slot();
        db.ledger()
            .read(move |c| {
                c.query_row(
                    "SELECT body FROM core_state WHERE kind='ryze_swap_nonce' AND id=?1",
                    params![k],
                    |r| r.get(0),
                )
                .optional()
            })
            .await
            .map_err(|e| e.to_string())
    }
    /// Сохраняет раздельные суммы исходного подписанного входа, возврата по пути и сторонних поступлений.
    pub async fn save_fill(&self, db: &Db, f: &super::chain::Mined) -> Result<bool, String> {
        let key = json!([self.digest, f.hash, f.receipt.block_hash]).to_string();
        let row=json!({"transaction":f.hash,"block":f.receipt.block_hash,"input":self.order.tokenIn,"output":self.order.tokenOut,
            "user_to_router":f.fill.input_to_router,"input_from_path":f.fill.input_from_path,"other_input_credit":f.fill.other_input_credit,
            "router_to_user":f.fill.output_from_router,"other_output_credit":f.fill.other_output_credit,"fee_in_input":f.fill.fee_in_input,
            "receipt_gas_units":f.receipt.gas_used,"receipt_gas_eth":f.receipt.gas_bnb()}).to_string();
        db.ledger().write(move|c|{
            c.execute("INSERT INTO core_state(kind,id,body,updated_at) VALUES('ryze_fill',?1,?2,?3) ON CONFLICT(kind,id) DO NOTHING",params![key,row,chrono::Utc::now().to_rfc3339()])?;
            c.query_row("SELECT body=?2 FROM core_state WHERE kind='ryze_fill' AND id=?1",params![key,row],|r|r.get(0))
        }).await.map_err(|e|e.to_string())
    }
    pub async fn release(&self, db: &Db) -> Result<(), String> {
        let k = self.slot();
        let id = self.claim();
        db.ledger()
            .write(move |c| {
                c.execute(
                    "DELETE FROM core_state WHERE kind='ryze_swap_nonce' AND id=?1 AND body=?2",
                    params![k, id],
                )
            })
            .await
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
}

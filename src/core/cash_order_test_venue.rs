//! Синтетическая биржа: тот же before_send, native accountId, частичный IOC и доказанное available.
use crate::{
    core::types::*,
    gateway::{
        contract::*, errors::*, events::MarketEvent, pending::PendingRef,
        settlement_intent as intent,
    },
    storage::cex_funds::{Balance as Cut, Booked, Bucket},
};
use rust_decimal::Decimal;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};
use tokio::sync::{Notify, mpsc};
pub(super) struct Spot {
    pub id: &'static str,
    pub posts: AtomicUsize,
    pub reads: AtomicUsize,
    pub balance: Mutex<Decimal>,
    pub fee_rate: Mutex<Decimal>,
    pub account: Mutex<String>,
    pub witnessed: Mutex<Vec<Booked>>,
    pub unknown: Mutex<bool>,
    pub reject_before_wire: Mutex<bool>,
    pub enter: Option<Arc<Notify>>,
    pub leave: Option<Arc<Notify>>,
}
impl Default for Spot {
    fn default() -> Self {
        Self {
            id: "bingx",
            posts: 0.into(),
            reads: 0.into(),
            balance: Mutex::new(100.into()),
            fee_rate: Mutex::new(Decimal::ZERO),
            account: Mutex::new("original-uid".into()),
            witnessed: Default::default(),
            unknown: Mutex::new(false),
            reject_before_wire: Mutex::new(false),
            enter: None,
            leave: None,
        }
    }
}
impl Spot {
    pub fn bucket(&self) -> Bucket {
        Bucket {
            venue: self.id().0,
            account: self.account.lock().unwrap().clone(),
            currency: "USDT".into(),
        }
    }
    fn fill(&self, req: TradeRequest) -> TradeResult {
        intent::note_gross(Some(30.into()));
        TradeResult {
            venue_ref: format!("exchange-{}", req.deal_id),
            request: req,
            status: TradeStatus::PartiallyFilled,
            filled_qty: 30.into(),
            avg_price: Decimal::ONE,
            fees: Fee::single(2.into(), "USDT"),
            chain: None,
            finished_at: chrono::Utc::now(),
            executor: None,
        }
    }
}
#[async_trait::async_trait]
impl Venue for Spot {
    fn id(&self) -> VenueId {
        VenueId(self.id.into())
    }
    fn kind(&self) -> VenueKind {
        VenueKind::Cex
    }
    fn supports_cash_reservation(&self) -> bool {
        true
    }
    fn supports_settlement_order(&self) -> bool {
        true
    }
    fn supports_settlement_credit(&self) -> bool {
        true
    }
    fn supports_settlement_withdrawal(&self) -> bool {
        true
    }
    fn settlement_account(&self) -> GatewayResult<String> {
        Ok("signature-owner".into())
    }
    fn cash_opening_cap(&self, _: &str, _: &str, q: Decimal, w: Decimal) -> Option<Decimal> {
        crate::venues::kucoin::test_cash_upper(q, w, *self.fee_rate.lock().unwrap())
    }
    fn cash_order_cap(&self, r: &TradeRequest, p: &PendingRef, c: &str) -> GatewayResult<Decimal> {
        Ok(self
            .cash_opening_cap(&r.asset.ticker, c, p.sent_qty.unwrap(), r.worst_price)
            .unwrap())
    }
    async fn cash_identity(&self, _: &str) -> GatewayResult<Bucket> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        Ok(self.bucket())
    }
    async fn cash_balance(
        &self,
        r: &TradeRequest,
        _: &PendingRef,
        _: &str,
    ) -> GatewayResult<(Cut, Decimal)> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        Ok((
            Cut {
                bucket: self.bucket(),
                epoch: 0,
                free: *self.balance.lock().unwrap(),
                booked: self.witnessed.lock().unwrap().clone(),
            },
            r.qty * r.worst_price,
        ))
    }
    async fn trade(&self, r: TradeRequest) -> GatewayResult<TradeResult> {
        let p = PendingRef::order(
            &format!("{}-USDT", r.asset.ticker),
            &format!("client-{}", r.deal_id),
            r.qty,
        );
        intent::before_send(&p).await?;
        crate::gateway::cash_signer::send_order(async {
            if *self.reject_before_wire.lock().unwrap() {
                return Err(GatewayError::new(
                    ErrorCode::NotConfigured,
                    Some(self.id()),
                    "ключ сменился до передачи запроса",
                ));
            }
            crate::gateway::cash_signer::dispatching();
            self.posts.fetch_add(1, Ordering::SeqCst);
            if let Some(w) = &self.enter {
                w.notify_one();
            }
            if let Some(w) = &self.leave {
                w.notified().await;
            }
            if *self.unknown.lock().unwrap() {
                return Err(GatewayError::new(
                    ErrorCode::OutcomeUnknown,
                    Some(self.id()),
                    "ответ утрачен",
                )
                .pending(p));
            }
            Ok(self.fill(r))
        })
        .await
    }
    async fn inspect_settlement_order(
        &self,
        r: TradeRequest,
        _: &PendingRef,
    ) -> GatewayResult<Option<TradeResult>> {
        if *self.unknown.lock().unwrap() {
            Ok(None)
        } else {
            Ok(Some(self.fill(r)))
        }
    }
    async fn check_connection(&self) -> GatewayResult<ConnectionCheck> {
        unreachable!()
    }
    async fn coin_networks(&self) -> GatewayResult<Vec<CoinNetworkInfo>> {
        unreachable!()
    }
    async fn subscribe_market(
        &self,
        _: Vec<Asset>,
        _: mpsc::Sender<MarketEvent>,
    ) -> GatewayResult<()> {
        unreachable!()
    }
    async fn quote(&self, _: &Asset, _: Side, _: Decimal) -> GatewayResult<Quote> {
        unreachable!()
    }
    async fn withdraw(&self, _: WithdrawRequest) -> GatewayResult<Transfer> {
        unreachable!()
    }
    async fn withdrawal_status(&self, _: &str) -> GatewayResult<Transfer> {
        unreachable!()
    }
    async fn deposit_address(&self, _: &Asset, _: &str) -> GatewayResult<String> {
        unreachable!()
    }
    async fn deposit_status(&self, _: &Asset, _: &str) -> GatewayResult<Transfer> {
        unreachable!()
    }
    async fn balances(&self) -> GatewayResult<Vec<Balance>> {
        unreachable!()
    }
    async fn withdraw_cost(&self, _: &Asset, _: &str, _: Decimal) -> GatewayResult<Vec<Cost>> {
        unreachable!()
    }
    async fn recheck_coins(&self, _: Vec<Asset>) -> GatewayResult<()> {
        unreachable!()
    }
    async fn dex_families(&self) -> GatewayResult<Vec<DexFamily>> {
        unreachable!()
    }
    async fn send_channels(&self) -> GatewayResult<Vec<SendChannelInfo>> {
        unreachable!()
    }
    async fn check_send_channel(&self, _: &str) -> GatewayResult<ConnectionCheck> {
        unreachable!()
    }
}

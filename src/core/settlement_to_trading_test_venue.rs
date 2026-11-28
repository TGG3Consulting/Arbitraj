//! Файл: core/settlement_to_trading_test_venue.rs
//! Что делает: синтетическая биржа для settlement_to_trading_tests.rs (того же вида, что cash_order_test_venue.rs):
//! фондовый счёт и спот одной монеты; deposit_to_trading перекладывает с фонда на спот или отвечает «мало средств»
//! (InsufficientBalance), как это переводит в единый код адаптер; режим «перекладывать нечего» — Ok(None).
//! Кто вызывает: settlement_to_trading_tests.rs.
//! От чего зависит: gateway/contract.rs (Venue), core/types.
use crate::{
    core::types::*,
    gateway::{contract::*, errors::*, events::MarketEvent},
};
use rust_decimal::Decimal;
use std::sync::{
    Mutex,
    atomic::{AtomicUsize, Ordering},
};
use tokio::sync::mpsc;

pub(super) struct Funds {
    pub fund: Mutex<Decimal>,
    pub spot: Mutex<Decimal>,
    /// true — у биржи перекладывать нечего (deposit_to_trading → Ok(None)).
    pub direct: bool,
    /// Вызовы deposit_to_trading.
    pub calls: AtomicUsize,
    /// Чтения торгового счёта (balances).
    pub reads: AtomicUsize,
}

impl Funds {
    /// Принимает: фонд, спот, режим «перекладывать нечего». Возвращает: биржу. Ошибок нет.
    pub fn new(fund: i64, spot: i64, direct: bool) -> Self {
        Self {
            fund: Mutex::new(fund.into()),
            spot: Mutex::new(spot.into()),
            direct,
            calls: 0.into(),
            reads: 0.into(),
        }
    }

    fn row(&self, free: Decimal) -> Balance {
        Balance {
            venue: self.id(),
            asset: "X".into(),
            free,
            locked: Decimal::ZERO,
        }
    }
}

#[async_trait::async_trait]
impl Venue for Funds {
    fn id(&self) -> VenueId {
        VenueId("bingx".into())
    }
    fn kind(&self) -> VenueKind {
        VenueKind::Cex
    }
    fn account_role(&self, account: &str) -> AccountRole {
        match account {
            "Спот" => AccountRole::Trading,
            "Фонд" => AccountRole::ToTrading,
            _ => AccountRole::Other,
        }
    }
    async fn deposit_to_trading(
        &self,
        _: &Asset,
        amount: Decimal,
    ) -> GatewayResult<Option<String>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.direct {
            return Ok(None);
        }
        let mut fund = self.fund.lock().unwrap();
        if *fund < amount {
            return Err(GatewayError::new(
                ErrorCode::InsufficientBalance,
                Some(self.id()),
                "мало средств на фондовом счёте",
            ));
        }
        *fund -= amount;
        *self.spot.lock().unwrap() += amount;
        Ok(Some(format!("{amount} X: фонд → спот")))
    }
    async fn account_balances(&self) -> GatewayResult<Vec<AccountBalances>> {
        let acc = |name: &str, free: Decimal| AccountBalances {
            account: name.into(),
            balances: vec![self.row(free)],
            error: None,
            note: None,
        };
        Ok(vec![
            acc("Спот", *self.spot.lock().unwrap()),
            acc("Фонд", *self.fund.lock().unwrap()),
        ])
    }
    async fn balances(&self) -> GatewayResult<Vec<Balance>> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        Ok(vec![self.row(*self.spot.lock().unwrap())])
    }
    async fn trade(&self, _: TradeRequest) -> GatewayResult<TradeResult> {
        unreachable!()
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

//! Файл: venues/bingx/mod.rs
//! Что делает: адаптер биржи BingX (спот) — реализация контракта gateway/contract.rs.
//! Всё общение с BingX — внутри этой папки; наружу — только типы ядра и единые ошибки шлюза.
//! Кто вызывает: main.rs (создаёт и регистрирует в реестре шлюза).
//! От чего зависит: файлы этой папки, gateway, settings, secrets, storage.
//! Настройки: venue.* с областью «bingx», feed.stale_after_ms; секреты bingx.api_key, bingx.api_secret.
//!
//! Только по официальной документации BingX (CLAUDE.md, решение 23). Ссылки — в шапке каждого файла
//! и в README.md этой папки.

pub mod account;
pub mod book;
pub mod ctx;
pub mod depth;
pub mod errors;
pub mod fees;
pub mod market;
pub mod num;
pub mod order_state;
pub mod rest;
pub mod symbols;
pub mod trade;
pub mod transfers;
pub mod wallet;
pub mod ws;
pub mod ws_conn;

use async_trait::async_trait;
use rust_decimal::Decimal;
use tokio::sync::mpsc;

use crate::core::types::{
    Asset, Balance, CoinNetworkInfo, Quote, Side, TradeRequest, TradeResult, Transfer, VenueId,
    WithdrawRequest,
};
use crate::gateway::contract::{ConnectionCheck, Venue, VenueKind};
use crate::gateway::errors::GatewayResult;
use crate::gateway::events::MarketEvent;

use book::Books;
use ctx::Ctx;
use fees::Fees;
use symbols::Symbols;

/// Адаптер BingX.
pub struct Bingx {
    ctx: Ctx,
    /// Задачи текущей подписки: при новой подписке старые останавливаются (без дублей соединений).
    tasks: std::sync::Mutex<Vec<tokio::task::JoinHandle<()>>>,
    symbols: Symbols,
    books: Books,
    fees: Fees,
}

impl Bingx {
    /// Создаёт адаптер. Ничего не подключает: соединения открываются по запросу ядра.
    pub fn new(ctx: Ctx) -> Self {
        Self {
            ctx,
            tasks: std::sync::Mutex::new(Vec::new()),
            symbols: Symbols::default(),
            books: Books::default(),
            fees: Fees::default(),
        }
    }
}

#[async_trait]
impl Venue for Bingx {
    fn id(&self) -> VenueId {
        self.ctx.venue()
    }

    fn kind(&self) -> VenueKind {
        VenueKind::Cex
    }

    async fn check_connection(&self) -> GatewayResult<ConnectionCheck> {
        Ok(account::check(&self.ctx).await)
    }

    async fn coin_networks(&self) -> GatewayResult<Vec<CoinNetworkInfo>> {
        wallet::coin_networks(&self.ctx).await
    }

    async fn subscribe_market(
        &self,
        assets: Vec<Asset>,
        out: mpsc::Sender<MarketEvent>,
    ) -> GatewayResult<()> {
        let new = market::subscribe(
            &self.ctx,
            &self.symbols,
            &self.books,
            &self.fees,
            assets,
            out,
        )
        .await?;
        let mut tasks = self.tasks.lock().unwrap_or_else(|p| p.into_inner());
        for old in tasks.drain(..) {
            old.abort();
        }
        *tasks = new;
        Ok(())
    }

    async fn quote(&self, asset: &Asset, side: Side, qty: Decimal) -> GatewayResult<Quote> {
        self.books
            .quote(&self.ctx, &self.fees, asset, side, qty)
            .await
    }

    async fn trade(&self, req: TradeRequest) -> GatewayResult<TradeResult> {
        self.symbols.ensure_fresh(&self.ctx).await?;
        trade::trade(&self.ctx, &self.symbols, req).await
    }

    async fn withdraw(&self, req: WithdrawRequest) -> GatewayResult<Transfer> {
        transfers::withdraw(&self.ctx, req).await
    }

    async fn withdrawal_status(&self, venue_ref: &str) -> GatewayResult<Transfer> {
        transfers::withdrawal_status(&self.ctx, venue_ref).await
    }

    async fn deposit_address(&self, asset: &Asset, network: &str) -> GatewayResult<String> {
        wallet::deposit_address(&self.ctx, asset, network).await
    }

    async fn deposit_status(&self, asset: &Asset, tx_hash: &str) -> GatewayResult<Transfer> {
        transfers::deposit_status(&self.ctx, asset, tx_hash).await
    }

    async fn balances(&self) -> GatewayResult<Vec<Balance>> {
        account::balances(&self.ctx).await
    }
}

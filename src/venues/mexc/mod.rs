//! Файл: venues/mexc/mod.rs
//! Что делает: адаптер биржи MEXC (спот, API v3) — реализация контракта gateway/contract.rs (решение 107).
//! Всё общение с MEXC — внутри этой папки; наружу — только типы ядра и единые ошибки шлюза.
//! Кто вызывает: main.rs (создаёт и регистрирует в реестре шлюза).
//! От чего зависит: файлы этой папки, gateway, settings, secrets, storage.
//! Настройки: venue.* с областью «mexc», feed.stale_after_ms; секреты mexc.api_key, mexc.api_secret.
//!
//! Только по официальной документации MEXC (https://www.mexc.com/api-docs/spot-v3/introduction). Ссылки — в шапке
//! каждого файла и в README.md этой папки. Фондового счёта у MEXC в документации нет: депозит сразу на споте.

pub mod account;
pub mod account_update;
pub mod book;
pub mod ctx;
pub mod depth;
pub mod errors;
pub mod fees;
pub mod market;
pub mod num;
pub mod order_state;
pub mod pairs;
pub mod pairs_watch;
pub mod proto;
pub mod rest;
pub mod subs;
pub mod symbols;
pub mod trade;
pub mod transfers;
pub mod user_key;
pub mod user_stream;
pub mod user_stream_conn;
pub mod wallet;
pub mod ws;
pub mod ws_conn;

use async_trait::async_trait;
use rust_decimal::Decimal;
use tokio::sync::mpsc;

use crate::core::types::{
    Asset, Balance, CoinNetworkInfo, Cost, Quote, SendChannelInfo, Side, TradeRequest, TradeResult,
    Transfer, VenueId, WithdrawRequest,
};
use crate::gateway::contract::{ConnectionCheck, DexFamily, Venue, VenueKind};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::gateway::events::MarketEvent;

use book::Books;
use ctx::Ctx;
use fees::Fees;
use subs::{Parts, Subs};
use symbols::Symbols;
use wallet::Networks;

/// Адаптер MEXC.
pub struct Mexc {
    ctx: Ctx,
    /// Текущая подписка: задачи соединений и обновление справочника пар (subs.rs).
    subs: Subs,
    symbols: Symbols,
    books: Books,
    fees: Fees,
    /// Коды сетей (netWork) → отображаемые имена (wallet.rs).
    nets: Networks,
}

impl Mexc {
    /// Создаёт адаптер. Ничего не подключает: соединения открываются по запросу ядра.
    pub fn new(ctx: Ctx) -> Self {
        Self {
            ctx,
            subs: Subs::default(),
            symbols: Symbols::default(),
            books: Books::default(),
            fees: Fees::default(),
            nets: Networks::default(),
        }
    }

    /// Ответ «не применимо» для функций контракта, которых у биржи нет (они только у DEX).
    /// Принимает: что именно не применимо. Возвращает: ошибку NotApplicable с площадкой MEXC.
    fn not_applicable(&self, what: &str) -> GatewayError {
        self.ctx.tag(GatewayError::new(
            ErrorCode::NotApplicable,
            None,
            format!("MEXC — биржа: {what} к ней не применимо"),
        ))
    }
}

#[async_trait]
impl Venue for Mexc {
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
        wallet::coin_networks(&self.ctx, &self.nets, &self.symbols).await
    }

    async fn subscribe_market(
        &self,
        assets: Vec<Asset>,
        out: mpsc::Sender<MarketEvent>,
    ) -> GatewayResult<()> {
        let parts = Parts {
            ctx: self.ctx.clone(),
            symbols: self.symbols.clone(),
            books: self.books.clone(),
            fees: self.fees.clone(),
        };
        self.subs.subscribe(&parts, assets, out).await
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
        wallet::deposit_address(&self.ctx, &self.nets, &self.symbols, asset, network).await
    }

    async fn deposit_status(&self, asset: &Asset, tx_hash: &str) -> GatewayResult<Transfer> {
        transfers::deposit_status(&self.ctx, asset, tx_hash).await
    }

    async fn balances(&self) -> GatewayResult<Vec<Balance>> {
        account::balances(&self.ctx).await
    }

    async fn deposit_to_trading(
        &self,
        asset: &Asset,
        amount: rust_decimal::Decimal,
    ) -> GatewayResult<Option<String>> {
        // Фондового счёта у MEXC в документации нет: депозит зачисляется на спот, переводить нечего.
        let _ = (asset, amount);
        Ok(None)
    }

    async fn withdraw_cost(
        &self,
        asset: &Asset,
        network: &str,
        _amount: Decimal,
    ) -> GatewayResult<Vec<Cost>> {
        wallet::withdraw_cost(&self.ctx, &self.nets, &self.symbols, asset, network).await
    }

    async fn recheck_coins(&self, _assets: Vec<Asset>) -> GatewayResult<()> {
        Err(self.not_applicable("проверка налогов токена и крючков пулов"))
    }

    async fn dex_families(&self) -> GatewayResult<Vec<DexFamily>> {
        Err(self.not_applicable("список видов пулов DEX"))
    }

    async fn send_channels(&self) -> GatewayResult<Vec<SendChannelInfo>> {
        Err(self.not_applicable("список каналов отправки транзакций"))
    }

    async fn check_send_channel(&self, _id: &str) -> GatewayResult<ConnectionCheck> {
        Err(self.not_applicable("проверка канала отправки транзакций"))
    }
}
